# F1 — Minimal native preview feasibility spike

Work package ID / status: **F1 / contract-tested** (macOS child overlay). **Not hardware-qualified.** Windows is open. F2 compositor/encoder is contract-tested; production playback stays unavailable (`previewAvailable: false`).

Commit + working-tree changes (including frontend tracking limitations): uncommitted implementation on 2026-09-08. Parent git may omit `front-end/` if that directory is still a `160000` gitlink; inventory frontend files explicitly before review.

OS / architecture / device / SDK / compiler / dependency versions: ARM64 macOS 26.6.2, Rust 1.98.1, Apple Swift 6.3.3. The bridge targets macOS 13. Tauri 2.x (`ns_window()` on `WebviewWindow`). No WGPU or FFmpeg dependency was added.

Fixture or real capture provenance / recording configuration: an in-process 128×128 solid-color H.264 MP4 written by `aeroshoot_preview_write_solid_mp4` (VideoToolbox / `AVAssetWriter`, Baseline AutoLevel, two frames). This is **not** a captured screen segment and **not** the existing `generate_valid_fmp4_segment` helper, which is not real H.264.

## Architecture decision

**Arrangement:** AppKit **child overlay** on the window `contentView`, sibling **above** WKWebView. React sends CSS points (`getBoundingClientRect` + `devicePixelRatio` + visibility + layout revision). The native adapter owns physical size, z-order, hit testing, and lifetime.

Rejected for this spike (no measurements that would justify them here): a separate preview window, an encoded stream over a custom protocol, and any path that serializes BGRA through Tauri invoke JSON.

**Ownership / threads:** `PreviewOwner` in Rust is the session. The Swift surface pointer is only used through `onMain` (run inline on the main thread, otherwise `DispatchQueue.main.sync`). Destroyed Tauri windows call `preview_detach`. Close/reopen bumps a generation; Swift attach takes that generation so a new view cannot accept an old present.

**IPC:** `preview_attach`, `preview_layout`, `preview_present_fixed` (RGB only), `preview_present_fixture` (**file path**, native decode), `preview_status`, `preview_hit_test`, `preview_detach`. Status JSON has no `pixels` or `samples` keys.

**HUD hit-through:** `hitMode: circle` — the view consumes the inscribed ellipse and returns `hitTest == nil` in the corners. This is same-window view hit testing, not a claim that clicks pass through a Tauri HUD to other applications.

## Reproduction commands

```sh
cargo test --manifest-path src-tauri/Cargo.toml
cargo check --manifest-path src-tauri/Cargo.toml --features tauri-app
sh script/test-native-preview.sh
# in front-end/
npm run build
```

Original spike manual checklist (not qualified): open a project in the desktop Edit Studio, confirm a colored AppKit rectangle tracks the placeholder, resize the window, switch scenes/close the window, and confirm no dangling overlay. Browser `npm run dev` must **not** be treated as F1 success.

Expected outcome: an owned native view can draw a fixed frame and present one decoded fixture without copying pixels through JS; stale layout/generation are rejected; HUD corners do not consume hits; detach/reattach leaves no Swift surface.

Observed outcome and measured values:

- **Copies:** bitmap presents copy once into a `CGImage` assigned to `CALayer.contents` (`copiesPerPresent: 1`). Fixed-color presents set `backgroundColor` only. Pixels never cross Tauri command/event IPC.
- **Memory / placement:** decode fixture is 128×128; status reports `arrangement: "child_overlay"` and physical size from CSS points × backing scale (for example 320×180 at scale 2 → 640×360).
- **Swift:** `sh script/test-native-preview.sh` — child overlay, stale revision/generation, occlusion hide, BGRA present, H.264 fixture decode, HUD circle hit-through, detach/reopen.
- **Rust:** 105 `cargo test` results; `preview_attach_impl` without a window errors; status omits pixel arrays; `PreviewOwner` rejects stale revisions/generations.
- **Frontend:** `npm run build`. Host is `front-end/src/components/canvas/NativePreviewHost.tsx` (no `<video>`). Browser emulation throws “requires the desktop app.”

Artifact paths (local/private where appropriate): in-test tempdirs (`TMPDIR` for the Swift runner); no production project files.

Checks not run and exact reason:

- Live Tauri window tracking against a running React layout (resize, occlusion, unrelated inspector/timeline clicks, close/reopen of the real `main` window).
- OS-wide click-through from a transparent `camera_overlay` HUD to other apps.
- Captured screen-segment decode, multi-stream compositing, audio clock, WGPU/FFmpeg (F2).
- Windows preview surface.

Open gates / next smallest package: **E4 — basic export and preview parity**. Z1 telemetry parsing may proceed independently. macOS live-window qualification and Windows media remain open.


## Review fixes — 2026-09-09

React now measures the preview rectangle and ancestor overflow clipping each animation frame, sending only changed geometry with one layout request in flight. This covers scroll and layout shifts that ResizeObserver alone misses. The AppKit view uses a clip mask and identical hit-test clipping. Hidden/occluded hosts are hidden natively. Attach/layout/detach carry a surface generation; stale teardown cannot remove a newer surface. The host remounts on project replacement. The E3 media worker presents actual composited project frames and marks previewAvailable only after successful native presentation.

`sh script/test-native-preview.sh` passed again, including child overlay, decoded fixture, clipped HUD hit testing and lifecycle contracts. Browser builds are not counted as native qualification. A temporary desktop validation bundle launched, but its new app identity could not be controlled by the available computer-use interface (actions returned “Computer Use is not active”). Live resize/scroll/click/close/reopen acceptance remains explicitly unqualified.


Repository handoff (2026-09-09): the broken frontend gitlink has been replaced by ordinary staged frontend source files. Existing implementation changes remain uncommitted. See REVIEW_E1_E4_F1_F2.md for the consolidated resolution and qualification limits.
