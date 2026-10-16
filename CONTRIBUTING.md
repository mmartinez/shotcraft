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

## Pull requests

Keep each pull request to one change.
Describe the problem, the fix, and how you verified it, with screenshots for UI changes.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in ShotCraft by you, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0, without any additional terms or conditions.
