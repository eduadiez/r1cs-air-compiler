# Third-party notices

The compiler's own code is licensed under `MIT OR Apache-2.0` (`LICENSE-MIT`,
`LICENSE-APACHE`). That license does not extend to the third-party files
below, which keep their own licenses. The SHA-256 hashes of all of them are
recorded in `docs/provenance.json`.

## PILout schema (`vendor/pilout`)

- `vendor/pilout/pilout.proto` is copied from the `pil2-compiler` package,
  version 0.14.0. The package pins the schema to commit
  `503862ca92a7493623bca1bc331cdbfb9398e435` of
  <https://github.com/0xPolygonHermez/pil2-compiler>.
- License: the package declares `MIT OR Apache-2.0`. Both license texts, with
  their copyright notices, are kept next to the schema
  (`vendor/pilout/LICENSE-MIT`, `vendor/pilout/LICENSE-APACHE`).
- The schema and both license files are identical to `src/pilout.proto`,
  `LICENSE-MIT` and `LICENSE-APACHE` at that commit.

## circomlib templates (`vendor/circomlib`)

- `vendor/circomlib/*.circom` and `vendor/circomlib/sha256/*.circom` are
  unmodified copies of circuit templates from circomlib, tag `v2.0.5`, commit
  `cff5ab6288b55ef23602221694a6a38a0239dcc0`:
  <https://github.com/iden3/circomlib>.
- License: their headers license them under the GNU General Public License,
  version 3 or (at your option) any later version. The package manifest says
  `GPL-3.0` and also covers `sha256/sha256.circom` and
  `sha256/sha256compression_function.circom`, which have no header.
- circomlib ships no license file, so `vendor/circomlib/COPYING` holds the
  GPL-3.0 text distributed with circom v2.2.3.

The fixtures `fixtures/range`, `fixtures/mux`, `fixtures/adder` and
`fixtures/sha256` include these templates, and their R1CS and WTNS files are
generated from them, so distributing those files is subject to the GPL. The
compiler itself never uses the templates, and the crate package leaves out
both the templates and the fixtures (see `include` in `Cargo.toml`). The
fixtures' own files (`kernel.circom`, inputs, layouts and expected hashes) are
part of this project and licensed like the compiler.

## Rust dependencies

Rust dependencies keep their own licenses; their exact versions are listed in
`Cargo.lock`.
