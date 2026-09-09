#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/aeroshoot-preview.XXXXXX")
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM
export TMPDIR="$test_dir"
xcrun swiftc -swift-version 5 -D PREVIEW_CONTRACT_TESTS \
  -target "$(uname -m)-apple-macosx13.0" \
  -module-cache-path "$test_dir/modules" \
  -framework AppKit -framework AVFoundation -framework QuartzCore -framework CoreVideo \
  "$repo_dir/src-tauri/native/macos/AeroShootPreview.swift" \
  "$repo_dir/src-tauri/native/macos/tests/PreviewContractTests.swift" \
  -o "$test_dir/preview-contract-tests"
"$test_dir/preview-contract-tests"
