# Icons

Placeholder. Real icons land in Phase 31d (polish + distribution).

For the dev / first build, run:

```bash
# From the kefilex-desk root
npx @tauri-apps/cli@latest icon path/to/source-square.png
```

This generates all the platform-specific sizes Tauri's `tauri.conf.json` expects (`32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`, `tray.png`).

Until then `tauri build` will fail with a missing-icon error. For purely-Rust validation (cargo check, cargo build --no-default-features), the icon files aren't needed.

A simple K-on-stone-background ~512x512 PNG is plenty for the initial generation. The 31d Notion sub-page tracks the polished asset.
