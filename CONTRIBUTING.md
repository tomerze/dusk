# Contributing to Dusk

Dusk is licensed under the GNU Affero General Public License, version 3
only ([LICENSE](LICENSE)). Every contribution to it is accepted under the
[Contributor Assignment Agreement](docs/docs/legal/cla.md), and how the
project is run is set out in [GOVERNANCE.md](GOVERNANCE.md). Read all
three before your first pull request.

## Before you contribute

### Sign the CLA

Every contributor signs the
[Contributor Assignment Agreement](docs/docs/legal/cla.md) once, from the
GitHub account they contribute with, before anything they wrote is merged.
Open a pull request; a check on it asks you to sign if you have not
already. Reply to it with the sentence it quotes, from that account. That
reply is your signature: it is recorded against the account and covers
every contribution you submit from it. Author your commits with an email
address your GitHub account lists, or the check cannot tell they are yours.
The sentence affirms that you are at least eighteen years old; if you are
not, do not reply, and open an issue instead so that a parent or guardian
can sign for you, as the agreement requires.

A company whose employees contribute arranges the Entity version of the
agreement (Harmony HA-CAA-E) in an issue first; once it is signed, the
owner adds the accounts it names to the check's allowlist, and those
accounts are not asked to sign individually. A pull request from an
account that has neither signed nor been listed is not merged, whatever
else it has going for it.

### Contribute only what you may assign

Code you wrote for an employer, code copied from another project, and code
generated from material under another license all need the rights sorted
out before they are submitted. If any part of a contribution is not your
own work, or you do not own the copyright in all of it, say so in the pull
request that submits it: name the part, where it came from, who owns it,
and the license it is under. The owner decides whether to accept it, and
the parts you do not own stay under their own license and are not assigned
by the agreement. Third-party material that cannot be assigned stays under
its own license, in its own directory, with its license file beside it, the
way the material the README lists already is.

### Open an issue first for anything that is not small

A bug fix or a documentation correction can go straight to a pull request.
Anything larger - a new feature, a change to a public interface, a new
dependency - is discussed in an issue first, so that no work is wasted on
a design that will not be accepted.

## Setting up

Install the Rust toolchain, the pre-commit hooks and the test runner:

```bash
rustup show
uv run pre-commit install
cargo install cargo-nextest --locked
```

Building also needs `make`, `cmake` and `autotools` for the vendored
Cap'n Proto compiler. [The development docs](docs/docs/development/index.md)
cover running a node, the CLI and the Python API, and
[their contributing page](docs/docs/development/contributing.md) says how
the code is written.

## Making a change

1. Fork the repository and branch from `master`.
2. Make the change, following the conventions in the development docs.
3. Keep each commit focused, with a subject line that says what it does.
4. Run the tests that cover your change with `cargo nextest run`. The
   pre-commit hooks run the formatters and linters on every commit.
5. Open a pull request against `master` and fill in the template.

## Review

The owner reviews every pull request and may ask for changes before it is
merged. Review covers whether the change fits the project, not only
whether it works. A pull request is merged as
[GOVERNANCE.md](GOVERNANCE.md#changes-to-the-tree) says.

## Reporting a security issue

Do not open a public issue for a vulnerability. Follow
[SECURITY.md](SECURITY.md).
