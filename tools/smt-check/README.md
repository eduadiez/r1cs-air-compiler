# SMT check

An optional program, run by hand, that proves for one packed artifact that its
stage-one constraints imply its R1CS. It is not part of the build, the tests
or CI, and it shares no code with the compiler or its checkers: it reads the
R1CS and PILout files itself, in Python with only the standard library, and
leaves what linear algebra cannot settle to an SMT solver.

```sh
air compile circuit.r1cs out/circuit --packed
tools/smt-check/check.py circuit.r1cs out/circuit [--solver COMMAND] [--timeout SECONDS]
```

It prints `proved: …` and exits with status 0, or lists each equation it could
not prove and exits with status 1.

## What it proves

For every source equation `Aᵢ · Bᵢ = Cᵢ`, and for every public binding, it
shows that no stage-one trace and public values can satisfy every gate and
public constraint of the PILout, with all cells of each copy cycle holding one
value, and violate it. So any trace that satisfies those constraints, and
whose copy cycles are constant, holds a satisfying assignment of the circuit
with the given public values.

- **Hypotheses.** Every constraint of the PILout whose expression reads only
  constants, fixed columns, stage-one columns and public values, instantiated
  on every row with the fixed column values of that row. Constraints over
  stage two or challenges, the permutation argument, are left out, which only
  weakens the hypotheses.
- **Copy cycles.** Read from the `ID` and `S{c}` fixed columns as SPEC.md
  defines them (cell `(r, c)` has label `ω^r · COSET^c`, and `S{c}` holds the
  label of the previous cell of its cycle). The program checks that `ID` is
  `ω^r`, that all labels are distinct, and that the `S` columns permute the
  labels of their own cells. A column without an `S` column has no copies.
- **Wires.** `map.bin` names the R1CS wire of each cell. It is the compiler's
  output, so the program only uses it to say which cycle holds which wire, and
  fails if the cells of one wire lie in different cycles. A wire in an
  equation must occupy a cell.

## How it proves

For each goal it takes the hypotheses that share variables with the goal,
widening the slice step by step (1, 2, 4, 8 steps, then all hypotheses): a
proof from fewer hypotheses is a proof from all.

1. **Linear elimination.** A hypothesis in which some variable appears only as
   a degree-one term gives that variable as a polynomial in the others; it is
   substituted everywhere. This keeps the same solutions.
2. **Linear combination.** If the goal is then a combination of the remaining
   hypotheses with constant coefficients, it vanishes wherever they do.
3. **Solver.** Otherwise the remaining hypotheses and the negated goal are sent
   to the solver, which must answer `unsat`. `sat` means that some trace
   violates the equation; `unknown`, or no answer within `--timeout` seconds,
   leaves the goal not proved.

On the `xor`, `adder`, `range` and `mux` fixtures every goal is proved by
steps 1 and 2; the solver is needed only for goals that are not linear
consequences, such as a changed equation. The program is pure Python: large
circuits take minutes (about 13 estimated for the `sha256` fixture).

## Solver

`COMMAND` (default `cvc5`) must read SMT-LIB from standard input and support
the finite-field theory `QF_FF`, as cvc5 1.0.8 and later do. With the Docker
image of [Picus](https://github.com/Veridise/Picus), which includes cvc5:

```sh
tools/smt-check/check.py circuit.r1cs out/circuit --solver "docker run --rm -i picus timeout 60 cvc5"
```

cvc5 does not always stop at its own time limits in this theory, so put a
`timeout` inside the container: killing the Docker client does not stop it.

## What it does not prove

- **The permutation argument.** That the stage-two constraints force each copy
  cycle to hold one value, except with negligible probability, is Lemmas 1
  and 2 of SPEC.md; the packed checker checks the exact form of those
  constraints and that each `S{c}` column is the one they read.
- **Completeness:** that every satisfying assignment gives a trace that
  satisfies the AIR. `tools/backend-check` shows it for the fixtures with a
  real prover.
- **The trusted parts of this program:** its R1CS and protobuf readers, its
  polynomial arithmetic and the solver.
