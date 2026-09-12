#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/aeroshoot-mic-gain.XXXXXX")
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM
xcrun swiftc -swift-version 5 -D MIC_GAIN_TESTS \
  -target "$(uname -m)-apple-macosx13.0" -module-cache-path "$test_dir/modules" \
  -framework AppKit -framework AVFoundation -framework ScreenCaptureKit -framework VideoToolbox -framework CoreMedia -framework CoreVideo -framework CoreImage \
  "$repo_dir/src-tauri/native/macos/AeroShootCapture.swift" \
  "$repo_dir/src-tauri/native/macos/AeroShootLivePreview.swift" \
  "$repo_dir/src-tauri/native/macos/MouseHookMac.swift" \
  "$repo_dir/src-tauri/native/macos/tests/MicGainContractTests.swift" -o "$test_dir/tests"
"$test_dir/tests"
