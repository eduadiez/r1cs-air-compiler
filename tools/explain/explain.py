#!/usr/bin/env python3
"""Draw how a circuit was compiled: one self-contained HTML page per artifact.

For a packed artifact, the page shows the trace cell by cell, coloured by
gate, and for each source equation its auxiliary wires and where each part is
placed; selecting a cell highlights every copy of its wire. For a reference or
module artifact, it shows the column of each wire and the equations.

The page is for reading, not for checking: it trusts the certificate and the
map, which `air check` verifies. This program reads only the documented file
formats and shares no code with the compiler.

Usage: explain.py SOURCE.r1cs ARTIFACT_DIR [--sym FILE.sym] [--witness DIR] [-o OUT.html] [--open]
ARTIFACT_DIR is the output directory of `air compile` or `air module`. With
--witness, the output directory of `air witness` for a packed artifact, the
page also shows the value in each cell. The
page is written to ARTIFACT_DIR/explain.html unless -o says otherwise. Signal
names come from the symbol file of `circom --sym`: by default SOURCE with the
.sym extension, if it exists.
"""
import argparse
import hashlib
import json
import struct
import sys
import webbrowser
from pathlib import Path

TEMPLATE = Path(__file__).with_name("page.html")


def fail(message):
    sys.exit(f"error: {message}")


def read_r1cs(path):
    """The prime, the counts of wires, outputs, public inputs and private
    inputs, and the equations of an R1CS version 1 file, each side a list of
    [wire, coefficient]."""
    data = Path(path).read_bytes()
    if data[:4] != b"r1cs" or struct.unpack_from("<I", data, 4)[0] != 1:
        fail(f"{path}: not an R1CS version 1 file")
    (count,) = struct.unpack_from("<I", data, 8)
    position, sections = 12, {}
    for _ in range(count):
        kind, length = struct.unpack_from("<IQ", data, position)
        sections.setdefault(kind, position + 12)
        position += 12 + length
    header = sections[1]
    (size,) = struct.unpack_from("<I", data, header)
    prime = int.from_bytes(data[header + 4 : header + 4 + size], "little")
    wires, outputs, inputs, private, _, constraints = struct.unpack_from(
        "<IIIIQI", data, header + 4 + size
    )
    at, equations = sections[2], []
    for _ in range(constraints):
        sides = []
        for _ in range(3):
            (terms,) = struct.unpack_from("<I", data, at)
            at += 4
            side = []
            for _ in range(terms):
                (wire,) = struct.unpack_from("<I", data, at)
                side.append([wire, int.from_bytes(data[at + 4 : at + 4 + size], "little")])
                at += 4 + size
            sides.append(side)
        equations.append(sides)
    return prime, (wires, outputs, inputs, private), equations


def read_names(path):
    """Wire -> signal name from a Circom .sym file (label,wire,component,name)."""
    names = {}
    for line in Path(path).read_text().splitlines():
        parts = line.split(",", 3)
        if len(parts) == 4 and parts[1].lstrip("-").isdigit() and int(parts[1]) >= 0:
            name = parts[3].removeprefix("main.")
            names.setdefault(int(parts[1]), name)
    return names


def signed(value, prime):
    """A field element for display: a small signed integer when it is one;
    else a power of two or a small multiple of a negative power of two, which
    is how Circom writes bit decompositions (8589934590 is -1/2^31 in
    Goldilocks); else the integer nearest to zero, as a string beyond 2^53,
    where JavaScript numbers lose precision."""
    value %= prime
    nearest = value - prime if value > prime // 2 else value
    if abs(nearest) < 2**16:
        return nearest
    for e in range(1, 192):
        power = pow(2, e, prime)
        if value in (power, prime - power):
            return f"{'-' if value != power else ''}2^{e}"
        scaled = value * power % prime
        scaled = scaled - prime if scaled > prime // 2 else scaled
        if abs(scaled) < 2**16:
            return f"{scaled}/2" if e == 1 else f"{scaled}/2^{e}"
    return nearest if abs(nearest) < 2**53 else str(nearest)


def side(terms, prime):
    return [[int(wire), signed(int(k), prime)] for wire, k in terms]


def plain(value, prime):
    """A trace value as the integer nearest to zero, as a string beyond 2^53."""
    value %= prime
    value = value - prime if value > prime // 2 else value
    return value if abs(value) < 2**53 else str(value)


def witness_values(directory, rows, width, publics, prime):
    """The stage-one trace and public values written by `air witness`."""
    trace, public = Path(directory) / "trace.bin", Path(directory) / "public.bin"
    for path in (trace, public):
        if not path.is_file():
            fail(f"{path}: missing; air witness SOURCE ARTIFACT_DIR INPUT.wtns {directory} writes it")
    data = trace.read_bytes()
    if len(data) != 8 * rows * width:
        fail(f"{trace} does not hold {rows} x {width} values; was it projected for this artifact?")
    values = public.read_bytes()
    if len(values) != 8 * publics:
        fail(f"{public} does not hold {publics} public values")
    cells = [plain(v, prime) for v in struct.unpack(f"<{rows * width}Q", data)]
    return cells, [plain(v, prime) for v in struct.unpack(f"<{publics}Q", values)]


def packed_data(certificate, prime, artifact):
    layout, rewritten = certificate["layout"], certificate["rewrittenCircuit"]
    rows, width = layout["rows"], layout["width"]
    if not (artifact / "map.bin").is_file():
        fail(f"{artifact} holds no map.bin; it is written by air compile with --packed or --layout")
    raw = (artifact / "map.bin").read_bytes()
    if len(raw) != 4 * rows * width:
        fail("map.bin does not match the certificate's rows and width")
    cells = list(struct.unpack(f"<{rows * width}I", raw))

    gate_of = {}
    for g, gate in enumerate(layout["gates"]):
        gate_of[(gate["gateType"], tuple(gate["columns"]))] = g
    cell_gate = [-1] * (rows * width)
    placements = []
    for p in layout["placements"]:
        g = gate_of.get((p["gateType"], tuple(p["columns"])), -1)
        for column in p["columns"]:
            cell_gate[p["row"] * width + column] = g
        placements.append([p["index"], g, p["row"], p["columns"], p["wires"]])
    gate_types = {t["id"]: t["width"] for t in layout["gateTypes"]}
    gates = [
        [gate["gateType"], gate_types.get(gate["gateType"], 0), gate["columns"],
         len(gate["rows"]), gate["rows"][0] if gate["rows"] else 0, gate["selector"]]
        for gate in layout["gates"]
    ]
    return {
        "rows": rows,
        "width": width,
        "usedRows": layout["usedRows"],
        "requiresPermutation": layout["requiresPermutation"],
        "cells": cells,
        "cellGate": cell_gate,
        "placements": placements,
        "gates": gates,
        "gateTypes": len(layout["gateTypes"]),
        "selectors": len(layout["selectors"]),
        "publicBindings": [[b["public"], b["wire"], b["row"], b["col"]] for b in layout["publicBindings"]],
        "originalWires": rewritten["originalWires"],
        "auxiliaries": [[a["id"], side(a["terms"], prime)] for a in rewritten["auxiliaries"]],
        "rewritten": [[side(s, prime) for s in eq] for eq in rewritten["equations"]],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("source")
    parser.add_argument("artifact")
    parser.add_argument("--sym", help="Circom symbol file (default: SOURCE with .sym)")
    parser.add_argument("-o", "--output", help="page to write (default: ARTIFACT_DIR/explain.html)")
    parser.add_argument("--witness", help="output directory of air witness, to show cell values")
    parser.add_argument("--open", action="store_true", help="open the page in a browser")
    args = parser.parse_args()

    source, artifact = Path(args.source), Path(args.artifact)
    if not source.is_file():
        fail(f"{source}: no such R1CS file")
    if not (artifact / "certificate.json").is_file():
        fail(
            f"{artifact} holds no certificate.json; create the artifact first, for example "
            f"with: air compile {source} {artifact} --packed"
        )
    try:
        certificate = json.loads((artifact / "certificate.json").read_text())
    except ValueError as error:
        fail(f"{artifact}/certificate.json: {error}")
    if certificate.get("sourceSha256") != hashlib.sha256(source.read_bytes()).hexdigest():
        fail(f"{artifact} was compiled from a different R1CS file than {source}")
    sym = Path(args.sym) if args.sym else source.with_suffix(".sym")
    if args.sym and not sym.is_file():
        fail(f"{sym}: no such symbol file")
    prime, (wires, outputs, inputs, private), equations = read_r1cs(source)
    data = {
        "source": source.name,
        "relation": certificate.get("relation", "unknown"),
        "wires": wires,
        "outputs": outputs,
        "publicInputs": inputs,
        "privateInputs": private,
        "names": {str(w): n for w, n in read_names(sym).items()} if sym.is_file() else {},
        "equations": [[side(s, prime) for s in eq] for eq in equations],
    }
    if "layout" in certificate:
        data["kind"] = "packed"
        data.update(packed_data(certificate, prime, artifact))
        if args.witness:
            data["values"], data["publicValues"] = witness_values(
                args.witness, data["rows"], data["width"], outputs + inputs, prime
            )
    elif "wireColumns" in certificate:
        data["kind"] = "module" if "exports" in certificate else "reference"
        data["wireColumns"] = certificate["wireColumns"]
        data["width"] = certificate["width"]
        data["rows"] = certificate.get("rows")
        data["publicWires"] = certificate.get("publicWires", [])
        data["exports"] = certificate.get("exports", [])
        if args.witness:
            fail("--witness needs a packed artifact: air witness projects only packed traces")
    else:
        fail("certificate.json is not a reference, packed or module certificate")

    payload = json.dumps(data, separators=(",", ":")).replace("</", "<\\/")
    page = TEMPLATE.read_text().replace("/*DATA*/null", payload)
    output = Path(args.output) if args.output else artifact / "explain.html"
    output.write_text(page)
    names = (
        f"signal names from {sym}"
        if data["names"]
        else "no signal names: pass --sym with the file of circom --sym"
    )
    print(f"wrote {output} ({len(page) // 1024} KiB, {data['kind']} artifact, {names})")
    if args.open:
        webbrowser.open(output.resolve().as_uri())


if __name__ == "__main__":
    main()
