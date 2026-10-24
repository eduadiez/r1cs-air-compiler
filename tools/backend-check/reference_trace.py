"""Write the stage-one trace and public values of a reference artifact.

Every row holds the full assignment of the circuit, with wire `w` in column
`wireColumns[w]`; the public values are the wires listed in `publicWires`.

Usage: reference_trace.py INPUT.wtns CERTIFICATE.json OUT_DIR
"""
import json
import os
import struct
import sys

GOLDILOCKS = 0xFFFFFFFF00000001


def read_witness(path):
    """Wire values of a WTNS version 2 file over the Goldilocks field."""
    with open(path, "rb") as file:
        data = file.read()
    magic, version, count = struct.unpack_from("<4sII", data, 0)
    if magic != b"wtns" or version != 2 or count != 2:
        sys.exit(f"{path}: not a WTNS version 2 file with two sections")
    position, sections = 12, {}
    for _ in range(count):
        tag, length = struct.unpack_from("<IQ", data, position)
        position += 12
        sections[tag] = data[position : position + length]
        position += length
    if sorted(sections) != [1, 2]:
        sys.exit(f"{path}: expected a header and a value section")
    size, prime, wires = struct.unpack_from("<IQI", sections[1], 0)
    if size != 8 or prime != GOLDILOCKS or len(sections[2]) != 8 * wires:
        sys.exit(f"{path}: not a Goldilocks witness with {wires} values")
    return list(struct.unpack(f"<{wires}Q", sections[2]))


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__.strip().splitlines()[-1])
    witness_path, certificate_path, out = sys.argv[1:]
    values = read_witness(witness_path)
    with open(certificate_path, encoding="utf-8") as file:
        certificate = json.load(file)
    if len(values) != len(certificate["wireColumns"]):
        sys.exit("the witness and the certificate have different wire counts")
    row = [0] * certificate["width"]
    for wire, column in enumerate(certificate["wireColumns"]):
        if column is not None:
            row[column] = values[wire]
    trace = row * certificate["rows"]
    publics = [values[wire] for wire in certificate["publicWires"]]
    with open(os.path.join(out, "trace.bin"), "wb") as file:
        file.write(struct.pack(f"<{len(trace)}Q", *trace))
    with open(os.path.join(out, "public.bin"), "wb") as file:
        file.write(struct.pack(f"<{len(publics)}Q", *publics))


if __name__ == "__main__":
    main()
