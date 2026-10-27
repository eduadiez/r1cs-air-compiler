#!/usr/bin/env python3
"""Draw how a circuit was compiled: one self-contained HTML page per artifact.

For a packed artifact, the page shows the trace cell by cell, coloured by
gate, and for each source equation its auxiliary wires and where each part is
placed; selecting a cell highlights every copy of its wire. For a reference or
module artifact, it shows the column of each wire and the equations.

The page is for reading, not for checking: it trusts the certificate and the
map, which `air check` verifies. This program reads only the documented file
formats and shares no code with the compiler.

Usage: explain.py SOURCE.r1cs ARTIFACT_DIR [--sym KERNEL.sym] [-o OUT.html]
ARTIFACT_DIR holds certificate.json, and map.bin for a packed artifact.
--sym takes the symbol file of `circom --sym`, to show signal names.
"""
import argparse
import json
import struct
import sys
from pathlib import Path

TEMPLATE = Path(__file__).with_name("page.html")


def fail(message):
    sys.exit(f"error: {message}")


def read_r1cs(path):
    """The prime, wire count, public wire count and equations of an R1CS
    version 1 file, each side a list of [wire, coefficient]."""
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
    wires, outputs, inputs, _, _, constraints = struct.unpack_from(
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
    return prime, wires, outputs + inputs, equations


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
    """A field element as a small signed integer when it is one, else as is."""
    value %= prime
    return value - prime if value > prime // 2 else value


def side(terms, prime):
    return [[int(wire), signed(int(k), prime)] for wire, k in terms]


def packed_data(certificate, prime, artifact):
    layout, rewritten = certificate["layout"], certificate["rewrittenCircuit"]
    rows, width = layout["rows"], layout["width"]
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
    parser.add_argument("--sym")
    parser.add_argument("-o", "--output", default="explain.html")
    args = parser.parse_args()

    artifact = Path(args.artifact)
    certificate = json.loads((artifact / "certificate.json").read_text())
    prime, wires, publics, equations = read_r1cs(args.source)
    data = {
        "source": Path(args.source).name,
        "relation": certificate.get("relation", "unknown"),
        "wires": wires,
        "publics": publics,
        "names": {str(w): n for w, n in read_names(args.sym).items()} if args.sym else {},
        "equations": [[side(s, prime) for s in eq] for eq in equations],
    }
    if "layout" in certificate:
        data["kind"] = "packed"
        data.update(packed_data(certificate, prime, artifact))
    elif "wireColumns" in certificate:
        data["kind"] = "module" if "exports" in certificate else "reference"
        data["wireColumns"] = certificate["wireColumns"]
        data["width"] = certificate["width"]
        data["rows"] = certificate.get("rows")
        data["publicWires"] = certificate.get("publicWires", [])
        data["exports"] = certificate.get("exports", [])
    else:
        fail("certificate.json is not a reference, packed or module certificate")

    payload = json.dumps(data, separators=(",", ":")).replace("</", "<\\/")
    page = TEMPLATE.read_text().replace("/*DATA*/null", payload)
    Path(args.output).write_text(page)
    print(f"wrote {args.output} ({len(page) // 1024} KiB)")


if __name__ == "__main__":
    main()
