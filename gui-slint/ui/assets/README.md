# Branding Assets

`logo.png` and `app-icon.png` are reused unchanged from
[GTAStudio/GTA-GameCheat](https://github.com/GTAStudio/GTA-GameCheat/tree/2866d3e4bb93cb45a132ec6bec9339db45897215/crates/ce-gui/ui/assets),
commit `2866d3e4bb93cb45a132ec6bec9339db45897215`.

The GUI follows that revision's `crates/ce-gui/ui/theme.slint` design tokens.
Images are embedded at build time; the installed application does not download
branding resources or require loose image files.

Windows builds generate an eight-size ICO (16, 20, 24, 32, 48, 64, 128, 256)
from the original application icon in Cargo's `OUT_DIR` and embed it into the EXE.

The `ui_tests` module renders all five pages at 1120x800 and 800x620 in both
languages and themes, checks the logo pixels, control bounds, clicks, language
selection, and switch alignment. It also renders the Azure form in both languages.
The 42 PNGs use `zh` / `en` filenames in `gui-slint/target/branding-screenshots`.
Run it with:

```powershell
cargo test --manifest-path gui-slint/Cargo.toml --locked --jobs 2 --target-dir gui-slint/target ui_tests::
```