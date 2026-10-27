# Architecture

The crate has three parts: the parser, the compiler (which builds artifacts)
and the checkers (which accept or reject them). Witness projection runs only
on a packed artifact the packed checker has accepted.

```text
api ──→ compiler side: compiler, dag, emit, module, rewrite, layout, packed
  └──→ checker side:  checker, module_check, packed_check, placement_check, pilout_check, polynomial, witness
shared by both:       r1cs, field, contract, certificate, error, protobuf schema
```

## Modules

All modules are in `src/`.

| Module | Side | Responsibility |
| --- | --- | --- |
| `api` | entry | Public functions: each compile function builds an artifact, then runs the checker on it |
| `error` | shared | The `Error` type: a message, its kind (invalid input or size limit) and the error that caused it |
| `r1cs` | shared | R1CS and WTNS parser |
| `field` | shared | Goldilocks arithmetic, the field element type and the two constants that define permutation labels |
| `contract` | shared | Constants only: relation, symbol, hint and constraint names, stages and size limits |
| `certificate` | shared | Certificate data (rewritten circuit, layout, the three certificate types, the composition binding) and accessors; no building or checking |
| `compiler`, `dag` | compiler | Reference PILout, with optional expression simplification and reuse (`dag`) |
| `emit` | compiler | PILout building blocks shared by the compilers: symbols, hints, encoding |
| `module` | compiler | Module fragment: the reference equations without row count, stage two or public values |
| `rewrite` | compiler | Auxiliary wires, and the gate type of each equation |
| `layout` | compiler | Placement of equations in cells: row groups and stacks |
| `packed` | compiler | Packed PILout: gate constraints, public bindings and the permutation argument, in that order |
| `polynomial` | checker | Exact polynomials and the expression evaluator |
| `pilout_check` | checker | Checks shared by both checkers: decoding, common PILout fields, symbols and hint layout |
| `checker` | checker | Reference checker |
| `module_check` | checker | Module checker, and the composition check of a host PILout |
| `placement_check` | checker | Checks that the auxiliary wires and the placement preserve the source equations and public order |
| `packed_check` | checker | Packed checker: rebuilds the expected PILout contents and compares |
| `witness` | checker | Checks a Circom witness and copies its values into the trace cells |
| `bin/air` | entry | The `air` command, a thin layer over the public API |

## Compiler and checkers

The compiler proposes an artifact; only the checker decides whether it is
accepted. The checker-side modules never import a compiler-side module, and
the test `checkers_do_not_use_compiler_modules` in `src/tests/mod.rs` fails if
they do. The reverse is allowed: `packed` runs `placement_check` on its own
placement, which is how `plan_layout` checks a plan.

The shared modules are trusted by both sides. Keep the two sides separate even
where their code looks similar: merging them would let a compiler bug hide
from the checker.

## Build

`build.rs` generates the Rust code for the protobuf schema
(`vendor/pilout/pilout.proto`) into Cargo's build directory; it is not
committed. A build needs only Rust, the locked crates and `protoc`.

## Tests

| Location | What it tests |
| --- | --- |
| `src/tests/` | Internal tests that need private access: corrupted artifacts and certificates, and property tests over random circuits |
| `tests/api.rs` | The public API |
| `tests/cli.rs` | The `air` executable and the fixtures |
| `fuzz/` | Fuzz targets, in their own package |

See [Testing](TESTING.md), [API](API.md), [CLI](CLI.md) and
[the specification](../SPEC.md).
