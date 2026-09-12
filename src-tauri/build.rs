use std::{env, path::PathBuf, process::Command};

fn main() {
    let skip_swift = env::var("AEROSHOOT_SKIP_SWIFT").is_ok();
    println!("cargo:rerun-if-env-changed=AEROSHOOT_SKIP_SWIFT");
    println!("cargo:rustc-check-cfg=cfg(stub_swift_ffi)");
    if skip_swift {
        println!("cargo:rustc-cfg=stub_swift_ffi");
    }
    if !skip_swift && env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        build_macos_capture_bridge();
    }

    #[cfg(feature = "tauri-app")]
    tauri_build::build();
}

fn build_macos_capture_bridge() {
    let source = PathBuf::from("native/macos/AeroShootCapture.swift");
    let live_source = PathBuf::from("native/macos/AeroShootLivePreview.swift");
    let mouse_source = PathBuf::from("native/macos/MouseHookMac.swift");
    let preview_source = PathBuf::from("native/macos/AeroShootPreview.swift");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", live_source.display());
    println!("cargo:rerun-if-changed={}", mouse_source.display());
    println!("cargo:rerun-if-changed={}", preview_source.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let library = out_dir.join("libaeroshoot_macos_capture.a");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").expect("Cargo target architecture");
    let swift_target = format!("{target_arch}-apple-macosx13.0");
    let module_cache = out_dir.join("swift-module-cache");
    std::fs::create_dir_all(&module_cache).expect("create Swift module cache");

    let status = Command::new("xcrun")
        .env("CLANG_MODULE_CACHE_PATH", &module_cache)
        .args([
            "swiftc",
            "-target",
            &swift_target,
            "-swift-version",
            "5",
            "-parse-as-library",
            "-O",
            "-emit-library",
            "-static",
            "-module-name",
            "AeroShootCapture",
        ])
        .arg(&source)
        .arg(&live_source)
        .arg(&mouse_source)
        .arg(&preview_source)
        .arg("-o")
        .arg(&library)
        .status()
        .expect("failed to invoke swiftc for the macOS capture bridge");
    assert!(
        status.success(),
        "swiftc failed to build the macOS capture bridge"
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=aeroshoot_macos_capture");
    for framework in [
        "AVFoundation",
        "AppKit",
        "CoreAudio",
        "CoreFoundation",
        "CoreGraphics",
        "CoreImage",
        "CoreMedia",
        "CoreVideo",
        "Foundation",
        "QuartzCore",
        "ScreenCaptureKit",
        "VideoToolbox",
        "AudioToolbox",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/Library/Developer/CommandLineTools/usr/lib/swift-5.5/macosx");
}
