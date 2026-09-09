#!/bin/sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/aeroshoot-mouse.XXXXXX")
trap 'rm -rf "$test_dir"' EXIT HUP INT TERM
xcrun swiftc -swift-version 5 -D MOUSE_CONTRACT_TESTS \
  -target "$(uname -m)-apple-macosx13.0" \
  -module-cache-path "$test_dir/modules" \
  "$repo_dir/src-tauri/native/macos/MouseHookMac.swift" \
  "$repo_dir/src-tauri/native/macos/tests/MouseHookContractTests.swift" \
  -o "$test_dir/mouse-contract-tests"
"$test_dir/mouse-contract-tests"
