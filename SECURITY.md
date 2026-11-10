# Security

The checkers are the trust boundary of this repository: an artifact they accept
must enforce every equation and public binding of its R1CS, as
[the specification](SPEC.md) states. Please report privately:

- an R1CS file and an artifact (PILout and certificate) that a checker accepts
  although the PILout does not enforce the R1CS;
- a host PILout that `check-composition` accepts although it does not enforce
  every equation of the module;
- a trace from `air witness` that satisfies the PILout for a witness that does
  not satisfy the R1CS;
- a crash, hang or unbounded allocation on input files, beyond the limits the
  CLI reports as `limit`.

Use [private vulnerability reporting](https://github.com/eduadiez/r1cs-air-compiler/security/advisories/new)
on GitHub, with the input files and the command that shows the problem. Ordinary
bugs, such as an artifact the checker wrongly rejects, can go in a public issue.

Before 1.0, only the latest commit on `main` receives fixes.
