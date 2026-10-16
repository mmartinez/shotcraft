# Contributing to ShotCraft

Bug reports, fixes and features are welcome.
Report security problems privately as described in [`SECURITY.md`](SECURITY.md), not in a public issue.

## Before you start

Read [`AGENTS.md`](AGENTS.md).
It describes the workspace layers and the rules every change follows: no panics outside tests, no `unsafe`, one renderer for preview and export, no webview UI, and clean-room work only.
For a large change, open an issue first so the approach is agreed before you build it.

## Building

The README's [Building](README.md#building) section lists the system packages.
The minimum supported Rust version is the `rust-version` in `Cargo.toml`.
CI type-checks the workspace with exactly that version, so a change that needs a newer compiler must raise it in the same pull request.

## Checks

Run these before you open a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI runs clippy and the tests on Linux, Windows and macOS and the format check on Linux, type-checks with the minimum Rust version, and validates the license list and the Linux desktop files.

## Dependencies and assets

After adding, removing or updating a dependency, regenerate the third-party license list and commit it with the change:

```sh
cargo install cargo-about --version 0.9.2 --locked --features cli
cargo about generate --workspace --locked about.hbs -o THIRD-PARTY-LICENSES.md
```

A new dependency's license must be in the `accepted` list in `about.toml`.
Bundled fonts, icons and images must be permissively licensed, keep their license file next to them, and get a row in [`ATTRIBUTION.md`](ATTRIBUTION.md).

## Tests

New behaviour comes with a test that fails without it: a boundary, an error path or an invariant a user would notice.
For UI changes, run the real binary and look at the result; AGENTS.md describes a headless X server setup for Linux.

### Mutation testing

[cargo-mutants](https://mutants.rs) changes the code in small ways (a flipped comparison, a function returning a default) and checks that a test fails.
CI runs it on the code a pull request changes and fails when a change survives.
Run the same check locally before you open the pull request:

```sh
cargo install cargo-mutants --version 27.1.0 --locked
git diff origin/main > /tmp/pr.diff
cargo mutants --in-diff /tmp/pr.diff
```

A surviving mutant usually means a missing test.
If the code cannot be unit tested because it needs a display or another process, add it to the exclusions in `.cargo/mutants.toml` in the same pull request.
A full run (`cargo mutants`) takes hours; the Mutants workflow runs it weekly and on demand, and reports the survivors without failing.

## Pull requests

Keep each pull request to one change.
Describe the problem, the fix, and how you verified it, with screenshots for UI changes.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in ShotCraft by you, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0, without any additional terms or conditions.
