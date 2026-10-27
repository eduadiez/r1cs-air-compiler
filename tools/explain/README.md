# Explain

An optional program, run by hand, that draws how a circuit was compiled: it
writes one self-contained HTML page for an artifact, to open in a browser. It
is not part of the build, the tests or CI. It is written in Python with only
the standard library, reads only the documented file formats, and shares no
code with the compiler.

From the repository root:

```sh
cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/packed --packed
tools/explain/explain.py fixtures/xor/kernel.r1cs out/packed --open
```

This writes `out/packed/explain.html` and opens it. The arguments are the R1CS
file and the directory that `air compile` (or `air module`) wrote; the program
stops with a message if that directory holds no artifact, or holds one
compiled from a different R1CS file.

| Option | Meaning |
| --- | --- |
| `--sym FILE` | Circom symbol file, so the page shows signal names (`aa.bit[6]`) instead of wire numbers (`w41`). By default, the R1CS path with the `.sym` extension, if that file exists, which is where `circom --r1cs --sym` puts it. It must come from the same Circom run as the R1CS file, since optimization renumbers the wires. |
| `-o FILE` | Where to write the page (default `ARTIFACT_DIR/explain.html`) |
| `--open` | Open the page in a browser |

## What the page shows

For a **packed** artifact:

- **Summary:** equations, auxiliary wires added, trace size, cells used,
  gates, selector columns, signals in several cells and public values.
- **Trace:** one box per cell, coloured by the gate (constraint) that covers
  it, with the signal it holds; a dot marks a public value. "Overview" shrinks
  the boxes to see the whole trace, and the buttons beside it zoom. Hovering a
  cell shows its signal and the equations placed on it; clicking it, or
  typing a signal name in the search box, outlines every cell that holds the
  signal, which the permutation argument makes equal.
- **Rows:** for each row, the equations placed on it, written with signal
  names, and the signal in each cell, 40 rows at a time.
- **Equations:** for each source equation, the equation, its form rewritten
  with auxiliary wires, the definition of each auxiliary wire, and where each
  part is placed, outlined in the trace.
- **Gates** (type, columns, rows and selector) and the signals with the most
  cells.

For a **reference** or **module** artifact: each equation over signals and
over columns, and the column of each signal.

Signals are named from the symbol file; without one, by role from the R1CS
header: `out[i]` outputs, `pub[i]` public inputs, `in[i]` private inputs and
`wN` other wires (`auxN` for auxiliary wires). The fixtures, except `sha256`,
include their `kernel.sym`. Coefficients are written as small integers, powers
of two or fractions over a power of two where they are one: Circom writes a
bit decomposition with coefficients such as `-1/2^31`, which is
`8589934590` in the Goldilocks field.

## Limits

The page displays the certificate and the map; it does not check them. Run
`air check` for that. For the `sha256` fixture the page is about 8 MB and
draws 107 520 cells.
