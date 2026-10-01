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

### Windows

Requirements: Windows 11 (x64 or ARM64), Node.js 22+, Rust with the MSVC toolchain (via [rustup](https://rustup.rs)), and the Visual Studio C++ build tools (the "Desktop development with C++" workload). The app uses the Microsoft Edge WebView2 Runtime, which ships with Windows 11.

```powershell
powershell -ExecutionPolicy Bypass -File .\script\codex.ps1 build
```

This installs the frontend dependencies if needed, runs the frontend and Rust tests, and builds the release app.

Output: `src-tauri\target\release\bundle\windows\AeroShoot.exe` and a zip of it (`AeroShoot_<version>_<arch>.zip`). The frontend is built into the exe, so no other files are needed next to it.

To build without running the tests:

```powershell
cd front-end; npm ci; npm run build; cd ..
cargo build --release --manifest-path src-tauri\Cargo.toml --features tauri-app,custom-protocol
```

The exe is then at `src-tauri\target\release\aeroshoot.exe`.
