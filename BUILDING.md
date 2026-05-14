# Building Kefilex Desk from source

For users / firms who'd rather build the binary themselves than trust the signed release.

## Prerequisites (Windows)

- Windows 10 (1809+) or Windows 11
- [Rust](https://rustup.rs/) toolchain (stable, latest)
- [Node.js 20+](https://nodejs.org/) and npm (for the Tauri WebView frontend shell)
- [Microsoft C++ build tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) — required for `windows-rs` crate
- [WebView2 runtime](https://developer.microsoft.com/microsoft-edge/webview2/) — preinstalled on Windows 11; auto-installs on Windows 10 if missing

## Clone & install

```powershell
git clone https://github.com/Kefilab/kefilex-desk.git
cd kefilex-desk
npm install
```

## Run in dev mode

```powershell
npm run tauri:dev
```

This opens the Preferences window and starts the tray-icon listener. Notifications flow through immediately; the WebSocket points at `http://localhost:3000/api/desk-companion/ws` by default (override with `KEFILEX_DESK_API` env var).

## Build a release binary

```powershell
npm run tauri:build
```

Output: `src-tauri/target/release/bundle/msi/KefilexDesk_x.y.z_x64_en-US.msi`

The binary is unsigned by default. To run a self-built binary you'll see a Windows SmartScreen warning ("publisher unknown") on first launch — that's expected. The signed binary served from app.kefilex.com is signed by SignPath.io with our EV certificate.

## Run the test suite

```powershell
cargo test
npm test
```

## Reproducible builds

The release pipeline pins all dependency versions in `Cargo.lock` (committed) and `package-lock.json`. A clean clone + the steps above on Windows 11 should produce a binary with an identical hash to the official release. Mismatches are tracked at [issues/reproducibility](https://github.com/Kefilab/kefilex-desk/issues?q=label%3Areproducibility).
