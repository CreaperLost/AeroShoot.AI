# AeroShoot.AI

A desktop screen recorder for macOS (13+, Apple Silicon) and Windows 11. It records the screen, a camera, a microphone, and computer audio as separate tracks, plus mouse movement, into a project the editor can open.

The companion video editor lives in its own repository: [`CreaperLost/Automated-Editor`](https://github.com/CreaperLost/Automated-Editor).

## Layout

- `front-end/` — React + Vite user interface.
- `src-tauri/` — Tauri 2 app and the Rust recording core.
  - `native/macos/` — Swift capture bridge (ScreenCaptureKit, AVFoundation).
  - `src/capture/windows/` — Windows capture (Windows Graphics Capture, Media Foundation, WASAPI).
- `script/` — build, test, and launcher scripts.

## Development

```sh
cd front-end && npm install && npm test
cd src-tauri && cargo test --no-default-features
```

To run the app in development mode, start the Vite dev server (`npm run dev` in `front-end/`), then run `cargo run --features tauri-app` in `src-tauri/`. On Windows, `script/codex.ps1 run` does both.

## Building the app

Both platforms build with Cargo directly; no Tauri CLI is needed. The frontend is embedded in the executable.

### macOS

Requirements: macOS 13 or newer on Apple Silicon, Node.js 22+, Rust (via [rustup](https://rustup.rs)), and the Xcode Command Line Tools (`xcode-select --install`) for the Swift capture bridge.

```sh
./script/build.sh            # AeroShoot.app
./script/build.sh --dmg      # also a DMG installer
./script/build.sh --install  # also install to /Applications
```

Output: `src-tauri/target/release/bundle/macos/AeroShoot.app` (and `bundle/dmg/` with `--dmg`).

The app is signed with the first code-signing identity found in your Keychain (Developer ID, Apple Development, or "AeroShoot Development"), or set `AEROSHOOT_SIGN_IDENTITY`. Without one it is signed ad hoc. A stable signature keeps the Screen Recording, Camera, Microphone, and Input Monitoring permissions across rebuilds.

`./script/codex.sh build` runs the frontend and Rust tests first, then the same build.

Without a Developer ID certificate the app is not notarized, so on another Mac the first launch needs right-click → Open (or System Settings → Privacy & Security → Open Anyway).

### Windows

Requirements: Windows 11 (x64 or ARM64), Node.js 22+, Rust with the MSVC toolchain (via [rustup](https://rustup.rs)), and the Visual Studio C++ build tools (the "Desktop development with C++" workload). The app uses the Microsoft Edge WebView2 Runtime, which ships with Windows 11.

```powershell
powershell -ExecutionPolicy Bypass -File .\script\codex.ps1 build
```

This installs the frontend dependencies if needed, runs the frontend and Rust tests, and builds the release app.

Output, under `src-tauri\target\release\bundle\`:

- `nsis\AeroShoot_<version>_x64-setup.exe`: the installer to share (recommended). It installs for the current user without an admin prompt, adds a Start menu shortcut and an uninstaller, and downloads WebView2 if it is missing.
- `msi\AeroShoot_<version>_x64_en-US.msi`: an MSI installer, for managed deployment.
- `windows\AeroShoot.exe` and `AeroShoot_<version>_<arch>.zip`: the app without an installer.

The installers need the Tauri CLI (`cargo install tauri-cli --locked`); without it, only the exe and zip are built. The installers are not code-signed, so Windows SmartScreen asks for confirmation the first time (More info → Run anyway).

To build without running the tests:

```powershell
cd front-end; npm ci; npm run build; cd ..
cargo build --release --manifest-path src-tauri\Cargo.toml --features tauri-app,custom-protocol
```

The exe is then at `src-tauri\target\release\aeroshoot.exe`.

### Installers from GitHub

The **Installers** workflow (`.github/workflows/release.yml`) builds the macOS DMG and the Windows setup.exe and MSI on GitHub's machines, so you don't need a Mac or a Windows PC for them. Run it from the repository's Actions tab and download the files from the run, or push a version tag to also publish them as a GitHub release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

Bump `version` in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` before tagging a new release.
