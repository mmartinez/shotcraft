# Attribution

Every non-code asset ShotCraft ships or fetches, with its author, source and license.
ShotCraft's own code is MIT OR Apache-2.0 (see [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)).
When you add an asset, add a row here in the same change.
Bundled third-party assets must be permissively licensed and keep their license file next to them.
Files downloaded at runtime are not redistributed; their row names the license that applies to them.

## Bundled in the app

| Path | Title | Author | Source | License |
|---|---|---|---|---|
| `crates/doc/assets/fonts/Inter-400.ttf`, `Inter-500.ttf`, `Inter-700.ttf` | Inter 4.x, Regular, Medium and Bold (UI and annotation text) | The Inter Project Authors (Rasmus Andersson) | <https://github.com/rsms/inter> | SIL OFL 1.1, [`crates/doc/assets/fonts/OFL.txt`](crates/doc/assets/fonts/OFL.txt) |
| `assets/app-icon/shotcraft.svg` and the PNGs generated from it | ShotCraft app icon | The ShotCraft contributors | This repository | MIT OR Apache-2.0 |

## Compiled in through dependencies

These come from crates.io packages and carry their licenses there.

| Asset | Crate | License |
|---|---|---|
| Phosphor icon font (regular weight) | `egui-phosphor` | MIT OR Apache-2.0 (icons: MIT, Phosphor Icons; notice in [`NOTICE`](NOTICE)) |
| Ubuntu Light and Noto Emoji (annotation text fallbacks), Hack (monospace) and Emoji Icon Font: egui's default fonts | `epaint_default_fonts` | Ubuntu Font Licence 1.0 (Ubuntu Light), SIL OFL 1.1 (Noto Emoji), MIT and Bitstream Vera License (Hack), MIT (Emoji Icon Font); Hack and Emoji Icon Font notices in [`NOTICE`](NOTICE) |

## Downloaded at runtime

Not redistributed: fetched on first use of text recognition into the user's data folder.

| File | Author | Source | License |
|---|---|---|---|
| `text-detection.onnx`, `text-recognition.onnx` | Robert Knight (ocrs) | <https://ocrs-models.s3-accelerate.amazonaws.com/>, model card at <https://huggingface.co/robertknight/ocrs> | CC BY-SA 4.0 (per the model card) |
