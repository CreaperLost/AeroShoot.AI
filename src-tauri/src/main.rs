// AeroShoot.AI Core Engine Main Entrypoint
// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "tauri-app")]
    {
        aeroshoot_lib::run();
    }

    #[cfg(not(feature = "tauri-app"))]
    {
        println!("▲ AeroShoot.AI — Core Recording Engine v0.1.0 (Headless Mode)");
        println!("  Phase 1: Shared Recording Foundations & Minimal Shell initialized.");
        println!("  To build the desktop GUI, compile with `--features tauri-app`.");
    }
}
