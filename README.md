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

`script/build.sh` builds the macOS app; `script/codex.ps1 build` builds the Windows one.
