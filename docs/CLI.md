# Command-line interface

Run `air --help` or `air COMMAND --help` for usage, and `air --version` for the
package version.

```text
air compile SOURCE OUT --rows N [--optimize]
air compile SOURCE OUT --layout CONFIG.json
air check SOURCE KERNEL.pilout CERTIFICATE.json
air plan SOURCE OUT --layout CONFIG.json
air witness SOURCE ARTIFACT_DIR INPUT.wtns OUT
```

No other argument forms are accepted. `OUT` must not exist and its parent
directory must exist: `OUT` is created, and an existing directory is never
overwritten.

`--rows N` sets the number of rows of a reference artifact: a power of two in
`64..=65536`.

| Command | Writes to `OUT` |
| --- | --- |
| `compile --rows` | `kernel.pilout`, `certificate.json` (reference) |
| `compile --layout` | `kernel.pilout`, `certificate.json` (packed), `map.bin` |
| `plan` | `plan.json`, `map.bin` |
| `witness` | `trace.bin`, `public.bin` |

`check` writes no files. It reads the certificate's `relation` field to decide
which checker to run. `witness` reads `kernel.pilout` and `certificate.json`
from `ARTIFACT_DIR`, which must hold a packed artifact.

`map.bin` holds the wire in each cell, row by row, as little-endian `u32`
values (0 for an unused cell). `trace.bin` and `public.bin` hold field elements
as little-endian `u64` values: the stage-one trace row by row, and the public
values in order. Certificates are the JSON form of the certificate types
described in [the specification](../SPEC.md); `certificate.json` and
`plan.json` are written as compact JSON followed by a newline.

## Layout configuration

`--layout` takes a JSON file such as `fixtures/xor/layout.json`:

```json
{"options":{"width":8,"chunk":4,"reuse":"prefix","fold":true},"strategy":"local","minRows":64}
```

| Field | Values | Meaning |
| --- | --- | --- |
| `options.width` | `3..=1024` | Columns per row, and the most distinct wires one equation may use |
| `options.chunk` | `2..width` | Terms replaced by each new helper wire |
| `options.reuse` | `none`, `prefix`, `available` | When an existing helper may stand in for terms of a later equation |
| `options.fold` | `true`, `false` | Rewrite `k · B = C` with a constant `k` as a linear equation first |
| `strategy` | `mixed`, `local`, `components` | How equations are assigned to rows |
| `minRows` | power of two, at least 64 | Smallest number of rows |

Unknown fields are rejected. The values of `reuse` and `strategy` mean:

- `reuse: none`: every split creates a new helper;
- `reuse: prefix`: reuse a helper whose terms are, up to a common factor, the
  terms being replaced;
- `reuse: available`: also search every side of the equation for the longest
  existing helper that appears there;
- `strategy: mixed`: put each equation in the fullest row that still has room;
- `strategy: local`: prefer a row that already holds some of the equation's wires;
- `strategy: components`: first place each group of wire-connected equations
  that fits in one row together, then continue as `local`.

## Reports

On success, a command prints one compact JSON object on stdout and exits with
status 0:

```json
{"command":"check","ok":true,"result":{"accepted":true}}
```

Keys are printed in alphabetical order; their order carries no meaning. The
`result` shown here is shortened. The fields of each result are:

| Command | `result` fields |
| --- | --- |
| `compile` | `mode` (`reference`, `optimized-reference` or `packed`), `piloutBytes`, `certificate` |
| `check` (reference) | `accepted`, `relation`, `sourceEquations`, `publicBindings`, `rows`, `width`, `formallyVerified` |
| `check` (packed) | `accepted`, `relation`, `sourceEquations`, `helpers`, `rows`, `width`, `requiresCopy`, `formallyVerified` |
| `plan` | `rows`, `width`, `helpers`, `requiresCopy`, `checkedEquations` |
| `witness` | `traceWords`, `publicWords` |

`formallyVerified` is always `false`: a passing check is not a formal proof.

On failure, a command prints one JSON object on stderr, nothing on stdout, and
exits with status 1:

```json
{"command":"compile","error":"read 'missing.r1cs': ...","kind":"io","ok":false}
```

`kind` tells why the command failed:

| `kind` | Meaning |
| --- | --- |
| `usage` | The arguments match no form of the command |
| `io` | A file could not be read or written, or an output directory could not be created (for example because it exists) |
| `invalid` | An input is malformed, unsupported or inconsistent, or an artifact does not encode its source's relation |
| `limit` | An input exceeds a size limit; a circuit too large for the reference layout may still fit a packed layout |

Help and version text go to stdout with status 0. Programs should rely on `ok`,
`command`, `kind` and the exit status. The error text names the file and the
failed check, and its wording may change.
