#!/usr/bin/env python3
"""Prove with an SMT solver that a packed artifact's stage-one constraints
imply its R1CS.

For every source equation, the solver is asked for a stage-one trace and
public values that satisfy every gate and public constraint of the PILout,
with all cells of a copy cycle holding one value, but violate the equation
(and likewise for each public binding). "unsat" proves that no such trace
exists. The copy cycles come from the PILout's ID and S fixed columns; the
artifact's map.bin only says which cell holds which R1CS wire, and must agree
with the cycles.

This program shares no code with the compiler or its checkers: it reads the
R1CS and PILout files itself and leaves the reasoning to the solver, cvc5 with
finite-field support. It does not check the permutation argument itself, the
claim that the stage-two constraints force each copy cycle to hold one value
(Lemmas 1 and 2 of SPEC.md).

Usage: check.py SOURCE.r1cs ARTIFACT_DIR [--solver COMMAND] [--timeout SECONDS]
ARTIFACT_DIR holds kernel.pilout and map.bin (air compile --packed or --layout).
COMMAND (default "cvc5") reads SMT-LIB from standard input, for example
"docker run --rm -i IMAGE cvc5".
"""
import argparse
import shlex
import struct
import subprocess
import sys
from collections import Counter
from pathlib import Path

# From SPEC.md, "Permutation argument": cell (r, c) has the label
# ω^r · COSET^c, with ω = 7^((P - 1) / n).
GENERATOR = 7
COSET = 12275445934081160404


def fail(message):
    sys.exit(f"error: {message}")


# ---------------------------------------------------------------- R1CS


def read_r1cs(path):
    """The prime, wire count, public wire count and equations (A, B, C as
    {wire: coefficient}) of an R1CS version 1 file."""
    data = Path(path).read_bytes()
    if data[:4] != b"r1cs" or struct.unpack_from("<I", data, 4)[0] != 1:
        fail(f"{path}: not an R1CS version 1 file")
    (count,) = struct.unpack_from("<I", data, 8)
    position, sections = 12, {}
    for _ in range(count):
        kind, length = struct.unpack_from("<IQ", data, position)
        sections.setdefault(kind, (position + 12, length))
        position += 12 + length
    header, _ = sections[1]
    (size,) = struct.unpack_from("<I", data, header)
    prime = int.from_bytes(data[header + 4 : header + 4 + size], "little")
    at = header + 4 + size
    wires, outputs, public_inputs, _, _, constraints = struct.unpack_from("<IIIIQI", data, at)
    at, _ = sections[2]
    equations = []
    for _ in range(constraints):
        sides = []
        for _ in range(3):
            (terms,) = struct.unpack_from("<I", data, at)
            at += 4
            side = {}
            for _ in range(terms):
                (wire,) = struct.unpack_from("<I", data, at)
                value = int.from_bytes(data[at + 4 : at + 4 + size], "little")
                side[wire] = (side.get(wire, 0) + value) % prime
                at += 4 + size
            sides.append(side)
        equations.append(sides)
    return prime, wires, outputs + public_inputs, equations


# ------------------------------------------------------- protobuf decoding


def varint(data, at):
    result = shift = 0
    while True:
        byte = data[at]
        at += 1
        result |= (byte & 0x7F) << shift
        if byte < 0x80:
            return result, at
        shift += 7


def message(data):
    """Fields of a protobuf message: {number: [value]}, where a value is an
    int (varint) or bytes (length-delimited). Packed repeated varints stay
    bytes; see `ints`."""
    fields, at = {}, 0
    while at < len(data):
        key, at = varint(data, at)
        number, wire_type = key >> 3, key & 7
        if wire_type == 0:
            value, at = varint(data, at)
        elif wire_type == 2:
            length, at = varint(data, at)
            value, at = data[at : at + length], at + length
        elif wire_type == 1:
            value, at = data[at : at + 8], at + 8
        elif wire_type == 5:
            value, at = data[at : at + 4], at + 4
        else:
            fail(f"unsupported protobuf wire type {wire_type}")
        fields.setdefault(number, []).append(value)
    return fields


def one(fields, number, default=None):
    values = fields.get(number)
    return values[-1] if values else default


def ints(fields, number):
    """A repeated varint field, packed or not."""
    result = []
    for value in fields.get(number, []):
        if isinstance(value, int):
            result.append(value)
        else:
            at = 0
            while at < len(value):
                item, at = varint(value, at)
                result.append(item)
    return result


def sint(value):
    return (value >> 1) ^ -(value & 1)


def element(data):
    return int.from_bytes(data, "big")


# PILout field numbers (vendor/pilout/pilout.proto).
OPERAND_KINDS = {
    1: "constant", 2: "challenge", 3: "proofValue", 4: "airGroupValue",
    5: "publicValue", 6: "periodicCol", 7: "fixedCol", 8: "witnessCol",
    9: "expression", 10: "airValue", 11: "customCol",
}


def operand(data):
    """An operand as (kind, fields)."""
    fields = message(data)
    ((number, (value,)),) = fields.items()
    return OPERAND_KINDS.get(number, "unknown"), message(value)


def read_pilout(path):
    pilout = message(Path(path).read_bytes())
    air_groups = pilout.get(3, [])
    if len(air_groups) != 1 or len(message(air_groups[0]).get(3, [])) != 1:
        fail("expected one AIR group with one AIR")
    air = message(message(air_groups[0])[3][0])
    expressions = []
    for data in air.get(6, []):
        ((number, (value,)),) = message(data).items()
        fields = message(value)
        kind = {1: "add", 2: "sub", 3: "mul", 4: "neg"}[number]
        if kind == "neg":
            expressions.append((kind, [operand(one(fields, 1))]))
        else:
            expressions.append((kind, [operand(one(fields, 1)), operand(one(fields, 2))]))
    constraints = []
    for data in air.get(7, []):
        ((number, (value,)),) = message(data).items()
        kind = {1: "firstRow", 2: "lastRow", 3: "everyRow", 4: "everyFrame"}[number]
        index = one(message(one(message(value), 1)), 1, 0)
        constraints.append((kind, index))
    symbols = []
    for data in pilout.get(11, []):
        fields = message(data)
        symbols.append((one(fields, 1, b"").decode(), one(fields, 4, 0), one(fields, 5, 0)))
    return {
        "prime": element(one(pilout, 2, b"")),
        "publics": one(pilout, 6, 0),
        "rows": one(air, 2, 0),
        "widths": ints(air, 5),
        "fixed": [[element(v) for v in message(c).get(1, [])] for c in air.get(4, [])],
        "expressions": expressions,
        "constraints": constraints,
        "symbols": symbols,
    }


# ----------------------------------------------------------- copy cycles


def copy_classes(pilout, prime):
    """For each cell (row-major), the index of its copy cycle, from the ID and
    S{c} fixed columns. A cell of a column without an S column is a cycle of
    its own."""
    rows, width = pilout["rows"], pilout["widths"][0]
    fixed_symbols = {name: index for name, kind, index in pilout["symbols"] if kind == 1}
    omega = pow(GENERATOR, (prime - 1) // rows, prime)
    label = {}
    for r in range(rows):
        row_label = pow(omega, r, prime)
        for c in range(width):
            label[row_label * pow(COSET, c, prime) % prime] = r * width + c
    if len(label) != rows * width:
        fail("cell labels are not distinct")
    successor = list(range(rows * width))
    if "Kernel.ID" in fixed_symbols:
        identity = pilout["fixed"][fixed_symbols["Kernel.ID"]]
        if identity != [pow(omega, r, prime) for r in range(rows)]:
            fail("the ID column is not ω^r")
        cells, previous = [], []
        for c in range(width):
            name = f"Kernel.S{c}"
            if name not in fixed_symbols:
                continue
            values = pilout["fixed"][fixed_symbols[name]]
            if len(values) != rows:
                fail(f"{name} does not have one value per row")
            for r, value in enumerate(values):
                if value not in label:
                    fail(f"{name} row {r} is not a cell label")
                # Cell (r, c) carries the label of the previous cell of its cycle.
                cells.append(r * width + c)
                previous.append(label[value])
        if sorted(previous) != sorted(cells):
            fail("the S columns do not permute the labels of their own cells")
        for cell, before in zip(cells, previous):
            successor[before] = cell
    classes, count = [None] * (rows * width), 0
    for start in range(rows * width):
        if classes[start] is None:
            cell = start
            while classes[cell] is None:
                classes[cell] = count
                cell = successor[cell]
            count += 1
    return classes


# ------------------------------------------------------------- polynomials


class Algebra:
    """Polynomials over the field as {monomial: coefficient}, a monomial being
    a sorted tuple of variable names."""

    def __init__(self, prime):
        self.p = prime

    def const(self, value):
        return {(): value % self.p} if value % self.p else {}

    def var(self, name):
        return {(name,): 1}

    def add(self, a, b, sign=1):
        result = dict(a)
        for monomial, k in b.items():
            value = (result.get(monomial, 0) + sign * k) % self.p
            if value:
                result[monomial] = value
            else:
                result.pop(monomial, None)
        return result

    def mul(self, a, b):
        result = {}
        for m1, k1 in a.items():
            for m2, k2 in b.items():
                monomial = tuple(sorted(m1 + m2))
                value = (result.get(monomial, 0) + k1 * k2) % self.p
                if value:
                    result[monomial] = value
                else:
                    result.pop(monomial, None)
        return result

    def substitute(self, polynomial, name, value):
        """`polynomial` with the variable `name` replaced by `value`."""
        if not any(name in monomial for monomial in polynomial):
            return polynomial
        result = {}
        for monomial, k in polynomial.items():
            rest = tuple(n for n in monomial if n != name)
            term = {rest: k}
            for _ in range(len(monomial) - len(rest)):
                term = self.mul(term, value)
            for m, coefficient in term.items():
                result[m] = (result.get(m, 0) + coefficient) % self.p
        return {m: k for m, k in result.items() if k}

    def eliminate(self, hypotheses, goal):
        """Use each hypothesis in which some variable appears only as a
        degree-one term to express that variable by the others, and substitute
        it everywhere. The hypotheses left and the goal have the same
        solutions as before, projected away from the eliminated variables."""
        hypotheses = [h for h in hypotheses if h]
        while True:
            best = None
            for i, h in enumerate(hypotheses):
                if best is not None and len(h) >= len(hypotheses[best[0]]):
                    continue
                counts = Counter(name for monomial in h for name in monomial)
                for monomial in h:
                    if len(monomial) == 1 and counts[monomial[0]] == 1:
                        best = (i, monomial[0])
                        break
            if best is None:
                return hypotheses, goal
            i, name = best
            h = hypotheses.pop(i)
            scale = (self.p - pow(h[(name,)], self.p - 2, self.p)) % self.p
            rest = {m: k for m, k in h.items() if m != (name,)}
            value = self.mul(rest, self.const(scale))
            hypotheses = [s for s in (self.substitute(g, name, value) for g in hypotheses) if s]
            goal = self.substitute(goal, name, value)

    def in_span(self, hypotheses, goal):
        """Whether `goal` is a combination of `hypotheses` with constant
        coefficients, so that it vanishes wherever they all do."""
        pivots = {}  # leading monomial -> reduced row with leading coefficient 1
        def reduce(row):
            row = dict(row)
            while row:
                lead = max(row)
                if lead not in pivots:
                    return row, lead
                row = self.add(row, self.mul(pivots[lead], self.const(row[lead])), -1)
            return row, None
        for h in hypotheses:
            row, lead = reduce(h)
            if row:
                inverse = pow(row[lead], self.p - 2, self.p)
                pivots[lead] = self.mul(row, self.const(inverse))
        return not reduce(goal)[0]


ALLOWED = {"constant", "fixedCol", "witnessCol", "publicValue", "expression"}


def stage_one_hypotheses(pilout, classes, algebra):
    """Each stage-one constraint instantiated on each row where it applies,
    as a nonzero polynomial over copy-cycle variables c* and public values
    p*. Constraints over stage two, challenges or other operands are left
    out, which only weakens the hypotheses."""
    rows, width = pilout["rows"], pilout["widths"][0]
    expressions = pilout["expressions"]
    allowed = {}

    def stage_one(index):
        if index not in allowed:
            allowed[index] = True
            ok = True
            for kind, fields in expressions[index][1]:
                if kind not in ALLOWED:
                    ok = False
                elif kind == "witnessCol" and one(fields, 1, 0) != 1:
                    ok = False
                elif kind == "expression" and not stage_one(one(fields, 1, 0)):
                    ok = False
            allowed[index] = ok
        return allowed[index]

    def value(op, row, memo):
        kind, fields = op
        if kind == "constant":
            return algebra.const(element(one(fields, 1, b"")))
        if kind == "fixedCol":
            column = pilout["fixed"][one(fields, 1, 0)]
            return algebra.const(column[(row + sint(one(fields, 2, 0))) % rows])
        if kind == "witnessCol":
            r = (row + sint(one(fields, 3, 0))) % rows
            return algebra.var(f"c{classes[r * width + one(fields, 2, 0)]}")
        if kind == "publicValue":
            return algebra.var(f"p{one(fields, 1, 0)}")
        return evaluate(one(fields, 1, 0), row, memo)

    def evaluate(index, row, memo):
        if index in memo:
            return memo[index]
        kind, ops = expressions[index]
        if kind == "neg":
            result = algebra.add({}, value(ops[0], row, memo), -1)
        elif kind == "mul":
            left = value(ops[0], row, memo)
            result = algebra.mul(left, value(ops[1], row, memo)) if left else {}
        else:
            left, right = value(ops[0], row, memo), value(ops[1], row, memo)
            result = algebra.add(left, right, 1 if kind == "add" else -1)
        memo[index] = result
        return result

    applies = {"firstRow": lambda r: r == 0, "lastRow": lambda r: r == rows - 1}
    chosen = [
        (applies.get(kind, lambda r: True), index)
        for kind, index in pilout["constraints"]
        if kind != "everyFrame" and stage_one(index)
    ]
    hypotheses = []
    for row in range(rows):
        memo = {}
        for on_row, index in chosen:
            if on_row(row):
                polynomial = evaluate(index, row, memo)
                if polynomial:
                    hypotheses.append(polynomial)
    return hypotheses


# ------------------------------------------------------------------ SMT


def smt_term(polynomial, prime):
    terms = []
    for monomial, k in sorted(polynomial.items()):
        factors = ([] if k == 1 and monomial else [f"(as ff{k} F)"]) + list(monomial)
        terms.append(factors[0] if len(factors) == 1 else f"(ff.mul {' '.join(factors)})")
    if not terms:
        return "(as ff0 F)"
    return terms[0] if len(terms) == 1 else f"(ff.add {' '.join(terms)})"


def query(prime, hypotheses, goal):
    names = set()
    for polynomial in hypotheses + [goal]:
        for monomial in polynomial:
            names.update(monomial)
    lines = ["(push 1)"]
    lines += [f"(declare-const {name} F)" for name in sorted(names)]
    lines += [f"(assert (= {smt_term(h, prime)} (as ff0 F)))" for h in hypotheses]
    lines.append(f"(assert (not (= {smt_term(goal, prime)} (as ff0 F))))")
    lines += ["(check-sat)", "(pop 1)"]
    return "\n".join(lines)


def solve(command, prime, hypotheses, goal, timeout):
    """The solver's answer for one query: "unsat" proves the goal; "unknown"
    also covers a solver that does not answer within `timeout` seconds."""
    script = "\n".join(
        [
            "(set-logic QF_FF)",
            f"(define-sort F () (_ FiniteField {prime}))",
            query(prime, hypotheses, goal),
        ]
    )
    try:
        process = subprocess.run(
            shlex.split(command) + ["--lang=smt2", "--incremental"],
            input=script,
            capture_output=True,
            text=True,
            check=False,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired:
        return "unknown"
    results = [word for word in process.stdout.split() if word in ("sat", "unsat", "unknown")]
    if len(results) == 1:
        return results[0]
    if process.returncode == 0:
        fail(f"unexpected solver output: {process.stdout[:200]}{process.stderr[:200]}")
    # The solver stopped without an answer, for example killed by a time limit.
    return "unknown"


def slice_hypotheses(hypotheses, by_variable, goal, depth):
    """The hypotheses reachable from the goal's variables in `depth` steps,
    or all of them when depth is None."""
    if depth is None:
        return hypotheses
    variables = {name for monomial in goal for name in monomial}
    chosen = set()
    for _ in range(depth):
        new = {i for name in variables for i in by_variable.get(name, ())} - chosen
        if not new:
            break
        chosen |= new
        variables |= {name for i in new for monomial in hypotheses[i] for name in monomial}
    return [hypotheses[i] for i in sorted(chosen)]


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("source")
    parser.add_argument("artifact")
    parser.add_argument("--solver", default="cvc5")
    parser.add_argument("--timeout", type=float, default=60.0)
    args = parser.parse_args()

    prime, wires, publics, equations = read_r1cs(args.source)
    pilout = read_pilout(Path(args.artifact) / "kernel.pilout")
    if pilout["prime"] != prime:
        fail("the R1CS and the PILout use different fields")
    if pilout["publics"] != publics:
        fail(f"the PILout has {pilout['publics']} public values, the R1CS {publics}")
    rows, width = pilout["rows"], pilout["widths"][0]
    data = (Path(args.artifact) / "map.bin").read_bytes()
    if len(data) != 4 * rows * width:
        fail("map.bin does not have one wire per cell")
    cell_wires = struct.unpack(f"<{rows * width}I", data)
    classes = copy_classes(pilout, prime)

    # The value of R1CS wire u is the copy cycle holding it; all its cells
    # must be in that one cycle.
    cycle = {}
    for cell, wire in enumerate(cell_wires):
        if wire:
            if cycle.setdefault(wire, classes[cell]) != classes[cell]:
                fail(f"the cells of wire {wire} are in different copy cycles")
    algebra = Algebra(prime)

    def wire_value(wire):
        if wire == 0:
            return algebra.const(1)
        if wire not in cycle:
            fail(f"R1CS wire {wire} occupies no cell")
        return algebra.var(f"c{cycle[wire]}")

    def linear(side):
        result = {}
        for wire, k in side.items():
            result = algebra.add(result, algebra.mul(algebra.const(k), wire_value(wire)))
        return result

    goals = []
    for index, (a, b, c) in enumerate(equations):
        goal = algebra.add(algebra.mul(linear(a), linear(b)), linear(c), -1)
        if goal:
            goals.append((f"equation {index}", goal))
    for j in range(publics):
        goals.append((f"public {j}", algebra.add(wire_value(j + 1), algebra.var(f"p{j}"), -1)))

    hypotheses = stage_one_hypotheses(pilout, classes, algebra)
    by_variable = {}
    for i, polynomial in enumerate(hypotheses):
        for name in {name for monomial in polynomial for name in monomial}:
            by_variable.setdefault(name, []).append(i)
    print(
        f"{len(equations)} equations, {publics} public values; "
        f"{len(hypotheses)} stage-one constraint instances over {rows} x {width} cells",
        file=sys.stderr,
    )

    # A proof from fewer hypotheses is a proof from all, so each goal first
    # gets small slices of the hypotheses. Linear hypotheses are eliminated
    # first; a goal that becomes 0, or a combination of the hypotheses left,
    # is proved without the solver. The solver gets the others, again from
    # small slices to all hypotheses.
    depths = (1, 2, 4, 8, None)
    pending = []
    for name, goal in goals:
        for depth in depths:
            chosen = slice_hypotheses(hypotheses, by_variable, goal, depth)
            left, reduced = algebra.eliminate(chosen, goal)
            if algebra.in_span(left, reduced):
                break
        else:
            pending.append((name, goal))
    print(
        f"{len(goals) - len(pending)} of {len(goals)} proved by linear algebra",
        file=sys.stderr,
    )
    unproved = []
    for name, goal in pending:
        for depth in depths:
            chosen = slice_hypotheses(hypotheses, by_variable, goal, depth)
            result = solve(args.solver, prime, *algebra.eliminate(chosen, goal), args.timeout)
            if result == "unsat":
                break
        else:
            meaning = "violated by some trace" if result == "sat" else "not decided"
            print(f"{name}: {meaning} ({result})")
            unproved.append(name)
    if pending:
        print(
            f"{len(pending) - len(unproved)} of {len(pending)} proved by the solver",
            file=sys.stderr,
        )
    if unproved:
        print(f"NOT PROVED: {len(unproved)} of {len(goals)}")
        sys.exit(1)
    print(f"proved: the stage-one constraints imply all {len(goals)} equations and public bindings")


if __name__ == "__main__":
    main()
