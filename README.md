# AeroShoot.AI

High-performance, AI-assisted screen recording studio for **macOS (Apple Silicon ARM)** and **Windows 11**.

Combining the low-level capture power of OBS Studio with the post-production elegance of Screen Studio and AI jump-cut automation.

**Status:** Shared recording foundations, a synthetic studio shell, and an in-progress macOS capture bridge exist. Phase completion and native qualification remain subject to the acceptance gates in the master plan, revised 2026-09-08. Native preview/export and platform performance are not yet qualified.

## 📖 Master Architecture & Development Roadmap
Please see [AEROSHOOT_MASTER_PLAN.md](AEROSHOOT_MASTER_PLAN.md) for the complete, persistent technical architecture, subsystem breakdown, and phased implementation guide.

### Key Highlights
- **Framework**: Tauri v2 (Rust shell) + React 18 / TypeScript / Vite / Tailwind CSS frontend.
- **Session & Epoch Clock**: Monotonic microsecond clock (`SessionEpoch`) bridging native capture timestamps with drift estimation.
- **Session State Machine**: Atomic, serialized transitions (`Idle -> Preparing -> Recording <-> Paused -> Stopping -> Completed`).
- **Non-Destructive Project Bundles**: Durable `.aero` project directories with append-only `journal.jsonl` and automatic crash recovery index rebuilding.
- **Smart Telemetry Logging**: Append-only cursor telemetry (`norm_x`, `norm_y`, `t_us`, `geometry_id`) decoupled from pixel encoding.
- **Shared Timeline & Ripple Cuts**: Non-destructive half-open retained intervals `[start_us, end_us)` mapping edited time to source media.
- **Audio DSP Silence Detection**: Vectorized RMS energy detector with configurable dBFS threshold, min duration, and syllable boundary padding.
- **Studio Interface**: Complete React studio containing Recording HUD, Floating Webcam Bubble HUD, Multi-Track Timeline with RMS waveforms, Canvas Background Customizer, and AI Jump-Cut modal.

## 🛠️ Development & Testing

### Running Rust Core Tests
```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

### Running Frontend Development Server
```bash
cd front-end
npm run dev
```

### Building Frontend
```bash
cd front-end
npm run build
```
