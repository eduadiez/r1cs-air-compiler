# Third-party notices

`vendor/pilout/pilout.proto` is copied from the `pil2-compiler` package,
version 0.14.0. The package pins the schema to upstream commit
`503862ca92a7493623bca1bc331cdbfb9398e435` of
<https://github.com/0xPolygonHermez/pil2-compiler>.

The package declares `MIT OR Apache-2.0`. Both license texts, with their
copyright notices, are kept next to the schema. The schema and both license
files are identical to `src/pilout.proto`, `LICENSE-MIT` and `LICENSE-APACHE`
at that commit, and their SHA-256 hashes are recorded in `docs/provenance.json`.

`vendor/circomlib/*.circom` are unmodified copies of circuit templates from
circomlib, tag `v2.0.5`, commit `cff5ab6288b55ef23602221694a6a38a0239dcc0`:
<https://github.com/iden3/circomlib>. Their headers license them under the GNU
General Public License, version 3 or (at your option) any later version; the
package manifest says `GPL-3.0`. circomlib ships no license file, so
`vendor/circomlib/COPYING` holds the GPL-3.0 text distributed with circom
v2.2.3. The fixtures in `fixtures/range`, `fixtures/mux` and `fixtures/adder`
include these templates; their R1CS and WTNS files are generated from them.
Distributing those files is subject to the GPL. The templates are used only by
fixtures, never by the compiler itself. The fixtures' own files
(`kernel.circom`, inputs, layouts and expected hashes) are part of this project
and licensed like the compiler.

Rust dependencies keep their own licenses; their exact versions are listed in
`Cargo.lock`.

The compiler's own code is licensed under `MIT OR Apache-2.0` (`LICENSE-MIT`,
`LICENSE-APACHE`). That license does not extend to the third-party files above,
which keep their own licenses.
