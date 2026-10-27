# Explain

An optional program, run by hand, that draws how a circuit was compiled: it
writes one self-contained HTML page for an artifact, to open in a browser. It
is not part of the build, the tests or CI. It is written in Python with only
the standard library, reads only the documented file formats, and shares no
code with the compiler.

From the repository root:

```sh
cargo run --bin air -- compile fixtures/xor/kernel.r1cs out/packed --packed
cargo run --bin air -- witness fixtures/xor/kernel.r1cs out/packed fixtures/xor/input.wtns out/witness
tools/explain/explain.py fixtures/xor/kernel.r1cs out/packed --witness out/witness --open
```

This writes `out/packed/explain.html` and opens it. The arguments are the R1CS
file and the directory that `air compile` (or `air module`) wrote; the program
stops with a message if that directory holds no artifact, or holds one
compiled from a different R1CS file.

| Option | Meaning |
| --- | --- |
| `--sym FILE` | Circom symbol file, so the page shows signal names (`aa.bit[6]`) instead of wire numbers (`w41`). By default, the R1CS path with the `.sym` extension, if that file exists, which is where `circom --r1cs --sym` puts it. It must come from the same Circom run as the R1CS file, since optimization renumbers the wires. |
| `--witness DIR` | The output directory of `air witness` for this artifact, so the page shows the value in each cell |
| `-o FILE` | Where to write the page (default `ARTIFACT_DIR/explain.html`) |
| `--open` | Open the page in a browser |

## What the page shows

For a **packed** artifact:

- **Summary:** equations, auxiliary wires added, trace size, cells used,
  gates, selector columns, signals in several cells and public values.
- **Trace:** one box per cell, coloured by the gate (constraint) that covers
  it, with row and column numbers; a dot marks a public value and a line marks
  where the padding rows start. The overview fits the whole trace in view and
  shows short values, such as bits; "Show names" widens the boxes to hold each
  cell's signal and value. Hovering a cell shows its signal, value and the
  equations placed on it.
- **Copy cycles:** clicking a cell, or typing a signal name in the search box,
  selects the signal and draws the numbered cycle through its cells: the
  cycle of labels along which the permutation argument makes them equal (the
  dashed link closes it).
- **Equations:** for each source equation, a diagram of how it is compiled:
  the equation, the parts it becomes (its rewritten form and each auxiliary
  wire, with the auxiliary wires each part uses), and the row and cells where
  each part is placed, outlined in the trace.
- **Rows:** for each row, the equations placed on it, written with signal
  names, and the signal and value in each cell, 40 rows at a time.
- **Gates** (type, columns, rows and selector) and the signals with the most
  cells.

An address ending in `#equation=31`, `#signal=both[2]` or `#view=names` opens
the page on that equation, signal or view.

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
