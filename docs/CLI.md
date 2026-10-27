# Command-line interface

`air --help` lists every command, `air COMMAND --help` shows one, and
`air --version` prints the package version.

```text
air compile SOURCE OUT [--rows N] [--optimize]
air compile SOURCE OUT --packed
air compile SOURCE OUT --layout CONFIG.json
air check SOURCE KERNEL.pilout CERTIFICATE.json
air plan SOURCE OUT [--layout CONFIG.json]
air witness SOURCE ARTIFACT_DIR INPUT.wtns OUT
air module SOURCE OUT --export WIRES [--optimize]
air check-composition SOURCE HOST.pilout CERTIFICATE.json BINDING.json
```

Arguments must appear exactly in these forms and in this order; anything else
fails with kind `usage`. `SOURCE` is an R1CS file.

- `OUT` must not exist, and its parent directory must. The command creates it
  only after the artifact has passed its check, and never overwrites an
  existing directory.
- `compile` without `--packed` or `--layout` builds a reference artifact.
  `--rows N` sets its row count, a power of two from 64 to 65536; without it
  the artifact has 64 rows, the fewest allowed, since every row holds a whole
  instance of the circuit. `--optimize` simplifies its expressions without changing the
  constraint polynomials.
- `compile --packed` builds a packed artifact with the layout the compiler
  chooses; `compile --layout CONFIG.json` with the settings in the file, the
  compiler choosing those it leaves out (see
  [Layout configuration](#layout-configuration)). `plan` does the same
  without generating PILout.
- `check` chooses the reference, packed or module checker from the
  certificate's `relation` field.
- `witness` reads `kernel.pilout` and `certificate.json` from `ARTIFACT_DIR`,
  which must hold a packed artifact (from `compile --packed` or `--layout`).
- `module` compiles a module for embedding in a host AIR. `--export WIRES` is
  a comma-separated list of distinct R1CS wire ids, for example `1,2,5`;
  `--optimize` simplifies expressions as for `compile`.
- `check-composition` checks that the final PILout of a host AIR preserves the
  module of `SOURCE` (`CERTIFICATE.json` from `module`) under `BINDING.json`
  (see [Composition binding](#composition-binding)).

## Output files

| Command | Files written to `OUT` |
| --- | --- |
| `compile` (reference) | `kernel.pilout`, `certificate.json` |
| `compile --packed`, `compile --layout` | `kernel.pilout`, `certificate.json`, `map.bin` |
| `plan` | `plan.json`, `map.bin` |
| `witness` | `trace.bin`, `public.bin` |
| `module` | `module.pilout`, `certificate.json` |
| `check`, `check-composition` | none |

| File | Content |
| --- | --- |
| `kernel.pilout` | The AIR, as PILout (protobuf) |
| `module.pilout` | The module fragment, as PILout (protobuf): constraint `i` is source equation `i`; not provable on its own |
| `certificate.json` | The certificate checked by `check`; its fields are listed in [the specification](../SPEC.md#certificates) |
| `plan.json` | The `rewrittenCircuit` and `layout` fields a packed certificate would have |
| `map.bin` | The wire in each cell, row by row, as little-endian `u32` (0 for an unused cell) |
| `trace.bin` | The stage-one trace, row by row, as little-endian `u64` field elements |
| `public.bin` | The public values in order, as little-endian `u64` field elements |

The JSON files are compact JSON followed by a newline.

## Composition binding

The binding says where a module's wires are in the host PILout. The host
writes it; every nonconstant wire of the R1CS must appear once, and no two
wires may share a column:

```json
{"airGroup":0,"air":0,"columns":[{"wire":1,"column":3},{"wire":2,"column":4}]}
```

- `airGroup`, `air`: indices of the host AIR group and AIR.
- `columns`: the host stage-one column of each wire, read at row offset 0.
  Placing the module's columns from host column `k` gives wire `i` the column
  `k + wireColumns[i]` (in the library, `CompositionBinding::with_offset`).

The check finds, for every source equation, an every-row constraint of that
AIR whose polynomial equals the equation over those columns. It accepts any
expression order and sharing and ignores the host's other columns,
constraints and hints. It establishes only that the AIR enforces the module's
equations on every row; see [the specification](../SPEC.md#module-relation-r1cs-module-per-row).

## Layout configuration

`--layout` takes a JSON file that sets some or all of the packed layout
settings, such as `fixtures/xor/layout.json`:

```json
{"width":8,"auxiliaryTerms":4,"reuseAuxiliaries":"prefix","foldConstants":true,"minRows":64,"rowGroupWidth":16}
```

Every field is optional; `{}` is the same as `--packed`. Unknown fields are
rejected.

| Field | Values | Default | Meaning |
| --- | --- | --- | --- |
| `width` | 3 to 1024 | 64 | The most distinct wires one equation may use |
| `auxiliaryTerms` | 2 to `width - 1` | half of `width`, at least 2 | Terms replaced by each new auxiliary wire |
| `reuseAuxiliaries` | `none`, `prefix`, `available` | `available` | When an existing auxiliary wire may stand in for terms of a later equation (see below) |
| `foldConstants` | `true`, `false` | `true` | If `A` or `B` in `A · B = C` is a constant `k`, rewrite the equation as the linear `0 = C - k · (other factor)` first |
| `minRows` | power of two, 64 to 65536 | chosen | The row count of the trace |
| `rowGroupWidth` | `width` to 1024 | chosen | The most wires in one row group (see below) |

An auxiliary wire is a new wire constrained to equal a linear combination of other
wires; auxiliary wires keep each equation within `width` wires.

`reuseAuxiliaries`:

- `none`: every split creates a new auxiliary wire.
- `prefix`: reuse an auxiliary wire whose terms, up to a common factor, are the terms
  being replaced.
- `available`: like `prefix`, and also search every side of the equation for
  the longest existing auxiliary wire that appears there.

Placement:

- Equations that share wires are joined into row groups of at most
  `rowGroupWidth` wires, when that lowers the estimated proving cost.
- The row groups of each kind (the same equations in the same positions) are
  stacked one per row in the same columns, so they share one constraint per
  gate.
- The trace has exactly `minRows` rows and as many columns as the stacks need,
  at most 1024. If more are needed the command fails with kind `limit`; a
  larger `minRows` gives a narrower trace.
- The columns are packed in two ways and the cheaper trace is kept (a column
  in the permutation argument counts 2.6 times). One way keeps cells that may
  need copies in their own columns, so the other columns stay out of the
  permutation argument.

A layout may have at most 65536 rows and 4 000 000 cells.

**Chosen settings.** When `minRows` or `rowGroupWidth` is left out, the
compiler builds the PILout of each candidate and keeps the one with the lowest
estimated proving time with pil2-proofman:

- `minRows`: 64, 128, … rows. Candidates whose trace exceeds a size limit are
  skipped; once one fits, the search stops when a candidate no longer fits or
  the estimate has risen twice in a row.
- `rowGroupWidth`: `width` and `2 · width` (up to 1024), each with its own
  search over `minRows`.

The estimate is a model of the prover's time without the FRI query phase,
fitted to 90 timed proofs of six circuits (median error 10%). It uses the
counts the prover's setup derives from the PILout: rows, committed and fixed
columns, and constraint operations. It ranks layouts and is not a measured
time; [Scale](SCALE.md#prover-time) compares it with measurements. The
`layout` field of the report gives the settings used, so a file holding it
builds the same artifact.

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
| `compile` | `mode` (`reference`, `optimized-reference` or `packed`), `relation`, `rows`, `width`, `piloutBytes`, `piloutSha256`; packed also `layout` (the settings used, every field filled in), `auxiliaries` and `requiresPermutation` |
| `check` (reference) | `accepted`, `relation`, `sourceEquations`, `publicBindings`, `rows`, `width`, `formallyVerified` |
| `check` (packed) | `accepted`, `relation`, `sourceEquations`, `auxiliaries`, `rows`, `width`, `requiresPermutation`, `formallyVerified` |
| `plan` | `layout` (as for `compile`), `rows`, `width`, `auxiliaries`, `requiresPermutation`, `checkedEquations` (auxiliary definitions plus source equations) |
| `witness` | `traceWords`, `publicWords` |
| `module` | `relation`, `width`, `exports`, `piloutBytes`, `piloutSha256` |
| `check` (module) | `accepted`, `relation`, `sourceEquations`, `width`, `exports` (count), `formallyVerified` |
| `check-composition` | `preserved`, `relation`, `claim`, `airGroup`, `air`, `airName`, `rows`, `sourceEquations`, `trivialEquations`, `exports` (each `wire` and its host `column`), `formallyVerified` |

`accepted` and `preserved` are always `true` (a rejected artifact gives an
error instead), and
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
