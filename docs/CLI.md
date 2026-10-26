# Command-line interface

`air --help` lists every command, `air COMMAND --help` shows one, and
`air --version` prints the package version.

```text
air compile SOURCE OUT --rows N [--optimize]
air compile SOURCE OUT --layout CONFIG.json
air check SOURCE KERNEL.pilout CERTIFICATE.json
air plan SOURCE OUT --layout CONFIG.json
air witness SOURCE ARTIFACT_DIR INPUT.wtns OUT
```

Arguments must appear exactly in these forms and in this order; anything else
fails with kind `usage`. `SOURCE` is an R1CS file.

- `OUT` must not exist, and its parent directory must. The command creates it
  only after the artifact has passed its check, and never overwrites an
  existing directory.
- `--rows N` sets the row count of a reference artifact: a power of two from
  64 to 65536.
- `check` chooses the reference or packed checker from the certificate's
  `relation` field.
- `witness` reads `kernel.pilout` and `certificate.json` from `ARTIFACT_DIR`,
  which must hold a packed artifact (from `compile --layout`).

## Output files

| Command | Files written to `OUT` |
| --- | --- |
| `compile --rows` | `kernel.pilout`, `certificate.json` |
| `compile --layout` | `kernel.pilout`, `certificate.json`, `map.bin` |
| `plan` | `plan.json`, `map.bin` |
| `witness` | `trace.bin`, `public.bin` |
| `check` | none |

| File | Content |
| --- | --- |
| `kernel.pilout` | The AIR, as PILout (protobuf) |
| `certificate.json` | The certificate checked by `check`; its fields are listed in [the specification](../SPEC.md#certificates) |
| `plan.json` | The `rewrittenCircuit` and `layout` fields a packed certificate would have |
| `map.bin` | The wire in each cell, row by row, as little-endian `u32` (0 for an unused cell) |
| `trace.bin` | The stage-one trace, row by row, as little-endian `u64` field elements |
| `public.bin` | The public values in order, as little-endian `u64` field elements |

The JSON files are compact JSON followed by a newline.

## Layout configuration

`--layout` takes a JSON file such as `fixtures/xor/layout.json`:

```json
{"options":{"width":8,"auxiliaryTerms":4,"reuseAuxiliaries":"prefix","foldConstants":true},"strategy":"local","minRows":64}
```

| Field | Values | Meaning |
| --- | --- | --- |
| `options.width` | 3 to 1024 | The most distinct wires one equation may use; with the row strategies, also the columns per row |
| `options.auxiliaryTerms` | 2 to `width - 1` | Terms replaced by each new auxiliary wire |
| `options.reuseAuxiliaries` | `none`, `prefix`, `available` | When an existing auxiliary wire may stand in for terms of a later equation (see below) |
| `options.foldConstants` | `true`, `false` | If `A` or `B` in `A · B = C` is a constant `k`, rewrite the equation as the linear `0 = C - k · (other factor)` first |
| `strategy` | `mixed`, `local`, `components`, `columns` | How equations are placed (see below) |
| `minRows` | power of two, 64 to 65536 | With `columns`, the row count; otherwise the least row count |
| `rowGroupWidth` | `options.width` to 1024 | Optional, `columns` only: the most wires in one row group. Defaults to `options.width` |

An auxiliary wire is a new wire constrained to equal a linear combination of other
wires; auxiliary wires keep each equation within `options.width` wires. Unknown fields
are rejected, and so is `rowGroupWidth` with a strategy other than `columns`.

`reuseAuxiliaries`:

- `none`: every split creates a new auxiliary wire.
- `prefix`: reuse an auxiliary wire whose terms, up to a common factor, are the terms
  being replaced.
- `available`: like `prefix`, and also search every side of the equation for
  the longest existing auxiliary wire that appears there.

`strategy`:

- `mixed`: put each equation in the fullest row that still has room.
- `local`: prefer a row that already holds some of the equation's wires.
- `components`: first place each group of wire-connected equations that fits
  in one row together, then continue as `local`.
- `columns`: join equations that share wires into row groups (at most
  `rowGroupWidth` wires each), then stack the row groups of each kind one per row
  in the same columns, so they share one constraint per gate. The trace has
  exactly `minRows` rows and as many columns as needed, at most 1024; if more
  are needed the command fails with kind `limit`, and a larger `minRows` may
  help. The columns are packed in two ways and the cheaper trace is kept (a
  column in the permutation argument counts 2.6 times); one way keeps cells that may
  need copies in their own columns, so the other columns stay out of the permutation
  argument.

With the row strategies (`mixed`, `local`, `components`), the trace has
`options.width` columns and the used rows rounded up to a power of two, but at
least `minRows`. Any layout may have at most 65536 rows and 4 000 000 cells.

`columns` gives wires more cells than the row strategies, but far fewer gate
constraints and selector columns, which usually makes proving faster.
[Scale](SCALE.md#prover-time) compares the strategies, and
`tools/backend-check/choose-layout.sh` estimates the best layout for a circuit.

## Reports

On success, a command prints one compact JSON object on stdout and exits with
status 0:

```json
{"command":"check","ok":true,"result":{"accepted":true,"formallyVerified":false,"publicBindings":3,"relation":"r1cs-per-row-publics-at-row-zero","rows":64,"sourceEquations":96,"width":96}}
```

Keys are printed in alphabetical order; their order carries no meaning. The
`result` fields are:

| Command | `result` fields |
| --- | --- |
| `compile` | `mode` (`reference`, `optimized-reference` or `packed`), `piloutBytes`, `certificate` (the whole certificate) |
| `check` (reference) | `accepted`, `relation`, `sourceEquations`, `publicBindings`, `rows`, `width`, `formallyVerified` |
| `check` (packed) | `accepted`, `relation`, `sourceEquations`, `auxiliaries`, `rows`, `width`, `requiresPermutation`, `formallyVerified` |
| `plan` | `rows`, `width`, `auxiliaries`, `requiresPermutation`, `checkedEquations` (auxiliary definitions plus source equations) |
| `witness` | `traceWords`, `publicWords` |

`accepted` is always `true` (a rejected artifact gives an error instead), and
`formallyVerified` is always `false`: a passing check is not a formal proof.

On failure, a command prints one JSON object on stderr, nothing on stdout, and
exits with status 1:

```json
{"command":"compile","error":"read 'missing.r1cs': No such file or directory (os error 2)","kind":"io","ok":false}
```

| `kind` | Meaning |
| --- | --- |
| `usage` | The arguments match no form of the command |
| `io` | A file could not be read or written, or `OUT` could not be created (for example because it exists) |
| `invalid` | An input is malformed, unsupported or inconsistent, or an artifact does not encode its source's relation |
| `limit` | An input exceeds a size limit; a circuit too large for the reference layout may still fit a packed layout |

Help and version text go to stdout with status 0. Programs should rely on `ok`,
`command`, `kind` and the exit status. The `error` text names the file and the
failed check, and its wording may change.
