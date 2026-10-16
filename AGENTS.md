# AGENTS.md: guide for AI agents and contributors

ShotCraft is a cross-platform screenshot tool written in Rust only.
The UI is native egui/eframe (glow renderer).
Never add Tauri, Electron or any webview or JavaScript UI layer.

## Workspace map

```text
crates/
  doc        L0 document model, tiny-skia rendering, pixel effects, scrolling-capture stitching (pure data and pixels)
  engine     L1 screen capture (xcap), settings and state, history, OCR (ocrs), clipboard, save, upload, launching modes
  ui-egui    L2 every window: capture overlay, editor, Quick Access, pin, scrolling capture, history, settings, toasts
apps/
  shotcraft  L3 the binary: clap CLI (one subcommand per mode) and the tray daemon
```

A crate depends only on lower layers.
Nothing below `ui-egui` may use egui, eframe, winit or rfd; `doc` uses `emath` and `ecolor` for geometry and colour.
File dialogs live in `ui-egui/src/dialogs.rs`.

Each mode runs in its own process (`shotcraft <command>`): the daemon and the windows launch modes with `engine::share::spawn`, so a crash in one capture never takes down the tray.

## Golden rules

### Never crash

A bad file, a corrupt settings file, a missing monitor or a full disk must produce an error the user can act on, never a panic.

- Non-test code never calls `unwrap()`, `expect()`, `panic!`, `unreachable!`, `todo!` or `unimplemented!`.
  Return the crate's error type and propagate with `?`, or use `ok_or`, `let .. else` and `if let`.
  The one exception is a provably infallible literal, marked `#[allow(clippy::expect_used)]` with a comment saying why it cannot fail.
- Every crate root carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]`; `clippy.toml` allows them in tests only.
- Use `get()` for indices that come from input, files, settings or arithmetic on them.
- Lock with `lock().unwrap_or_else(PoisonError::into_inner)` and treat thread joins as `Result`s.
- Pixel buffers sized by input go through `render::pixmap`, which refuses sizes over its cap with `doc::Error::TooLarge`; tiny-skia aborts the process on a failed allocation, so never call `Pixmap::new` directly.
- `apps/shotcraft/src/main.rs` catches an escaped panic around `dispatch` and reports it; keep `panic = "unwind"`.

### No `unsafe`

The workspace sets `unsafe_code = "forbid"`.
Platform interop that winit and egui do not cover goes in `ui-egui/src/platform.rs` and uses safe bindings (objc2 on macOS); capture-level queries the engine needs below the UI (the cursor position) stay in `engine/src/capture.rs`.

### Errors

`doc` and `engine` return their own `thiserror` types (`shotcraft_doc::Error`, `shotcraft_engine::Error`) so callers and future automation get structured errors.
`ui-egui` and the app use `anyhow` for window and flow glue.

### Clean-room

Never download, decompile or copy another product's binaries, code or assets.
Bundled third-party assets must be permissively licensed, keep their license file next to them and get a row in `ATTRIBUTION.md` in the same change.
Files the app downloads at runtime instead of bundling (the OCR models, CC BY-SA 4.0) are not redistributed, so the permissive rule does not apply; they still get a row in `ATTRIBUTION.md` naming their license.

### Licenses and icons

`THIRD-PARTY-LICENSES.md` is generated; never edit it by hand.
After changing dependencies, regenerate it with `cargo about generate --workspace --locked about.hbs -o THIRD-PARTY-LICENSES.md` (cargo-about 0.9.2); CI fails when it is stale or a crate's license is not in `about.toml`'s `accepted` list.
Add a license to `accepted` only if it is permissive or, like MPL-2.0, file-level copyleft that applies to unmodified crates.

The app icon's master is `assets/app-icon/shotcraft.svg`; after editing it, run `packaging/icons.sh` (needs resvg) and commit the PNGs it writes.
The app id `io.github.mmartinez.shotcraft` (`theme::APP_ID`) names the window class, the desktop entry and the icons; change them together.

### Shared rendering

The editor preview and every export go through `doc::render`, so what the user sees is what gets saved.
Add drawing behaviour there, not in egui painting code.

## Checks

Run these before every commit; CI runs clippy and the tests on Linux, Windows and macOS, and the format check on Linux.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

CI also type-checks the workspace (`cargo check`) with the `rust-version` in `Cargo.toml` (the egui crates set that floor); raise it in the same change as code that needs a newer compiler.

Linux build packages are listed in the README's Building section; CI installs the same list.
For platform-specific code, also check the other targets, for example with `cargo zigbuild clippy --target aarch64-apple-darwin` (ring's C code needs a cross C toolchain, which zig provides).

## Verifying UI changes

Run the real binary and look at the result.
On Linux a headless X server works: start `Xvfb :77 -screen 0 1920x1080x24` and a window manager, point `DISPLAY`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` at a scratch directory, launch subcommands, drive them with XTest, and screenshot the root window.
Give egui at least 50 ms between synthetic input events or it misses keys.

## Next: everything is a command

The planned next step is a command registry in `engine` (stable id, label, shortcut, JSON params), dispatched by id from the UI, the CLI, a JSON control channel and an MCP server.
New user-visible behaviour should be shaped so it can become such a command.
