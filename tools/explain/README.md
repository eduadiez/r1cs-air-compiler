# Explain

An optional program, run by hand, that draws how a circuit was compiled: it
writes one self-contained HTML page for an artifact, to open in a browser. It
is not part of the build, the tests or CI. It is written in Python with only
the standard library, reads only the documented file formats, and shares no
code with the compiler.

```sh
air compile circuit.r1cs out/circuit --packed
tools/explain/explain.py circuit.r1cs out/circuit --sym circuit.sym -o circuit.html
```

`--sym` is optional: it takes the symbol file that `circom --sym` writes, so
that the page shows signal names (`aa.bit[6]`) instead of wire numbers
(`w41`). Write it in the same Circom run as the R1CS file, with the same
`--O` level, since optimization renumbers the wires.

## What the page shows

For a **packed** artifact:

- a summary: equations, auxiliary wires, trace size, cells used, gates,
  selectors, copied wires and public values;
- the trace, one square per stage-one cell, coloured by the gate whose
  constraint covers it, with the cells of public values marked. Hovering a
  cell shows its wire and the equations placed there; clicking it highlights
  every cell of that wire, which the permutation argument makes equal;
- for each source equation: the equation, its rewritten form, the auxiliary
  wires it uses with their definitions, and the row, columns and gate of each
  part, highlighted in the trace;
- the gates (type, columns, rows and selector) and the wires with the most
  cells.

For a **reference** or **module** artifact: the column of each wire, the
public or exported wires, and each equation over wires and over columns.

## Limits

The page displays the certificate and the map; it does not check them. Run
`air check` for that. Coefficients are shown as signed integers when they are
small, so `-1` stands for `P - 1`. For the `sha256` fixture the page is about
8 MB and draws 107 520 cells.
