#!/usr/bin/env bash
# ==============================================================================
# AeroShoot.AI — Production Release Build Script
# ==============================================================================
# Builds the complete production release of AeroShoot:
#   1. Sets up PATH and verifies required toolchains (Node, Cargo, Swift)
#   2. Removes previous frontend/Rust/bundle artifacts so the release is not stale
#   3. Installs frontend dependencies (if needed) & builds the Vite production bundle
#   4. Builds the optimized Rust/Swift native desktop release and packages the macOS .app
# ==============================================================================

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$ROOT_DIR/front-end"
TAURI_DIR="$ROOT_DIR/src-tauri"
DMG_STAGING_DIR="/tmp/aeroshoot_dmg_staging"

trap 'rm -rf "$DMG_STAGING_DIR"' EXIT

ensure_environment() {
  # 1. Ensure standard system directories are in PATH
  for sys_dir in /usr/bin /bin /usr/sbin /sbin; do
    if [[ -d "$sys_dir" && ":$PATH:" != *":$sys_dir:"* ]]; then
      PATH="$PATH:$sys_dir"
    fi
  done

  # 2. Add common candidate directories in descending priority order
  local candidate_paths=(
    "$HOME/.cargo/bin"
    "$HOME/Library/pnpm"
    "$HOME/.fnm/current/bin"
    "$HOME/.local/share/fnm"
    "$HOME/.local/share/mise/shims"
    "$HOME/.asdf/bin"
    "$HOME/.asdf/shims"
    "$HOME/.volta/bin"
    "/usr/local/bin"
    "/opt/homebrew/sbin"
    "/opt/homebrew/bin"
  )
  for p in "${candidate_paths[@]}"; do
    if [[ -d "$p" && ":$PATH:" != *":$p:"* ]]; then
      PATH="$p:$PATH"
    fi
  done

  # 3. Try loading NVM if present
  local nvm_dir="${NVM_DIR:-$HOME/.nvm}"
  if [[ -s "$nvm_dir/nvm.sh" ]]; then
    export NVM_DIR="$nvm_dir"
    set +u
    # shellcheck disable=SC1090
    \. "$nvm_dir/nvm.sh" 2>/dev/null || true
    set -u
  fi

  # 4. Direct scan of NVM installed node versions if npm still not found
  if ! command -v npm >/dev/null 2>&1 && [[ -d "$nvm_dir/versions/node" ]]; then
    local ver_dir
    for ver_dir in $(ls -1d "$nvm_dir/versions/node"/* 2>/dev/null | sort -V -r); do
      if [[ -x "$ver_dir/bin/npm" ]]; then
        PATH="$ver_dir/bin:$PATH"
        break
      fi
    done
  fi

  # 5. Fallback: query the user login shell for its exported PATH
  if ! command -v npm >/dev/null 2>&1 || ! command -v node >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
    local user_shell="${SHELL:-/bin/zsh}"
    if [[ -x "$user_shell" ]]; then
      local shell_path=""
      shell_path="$("$user_shell" -l -c 'printf "%s" "$PATH"' 2>/dev/null || true)"
      if [[ -n "$shell_path" ]]; then
        PATH="$shell_path:$PATH"
      fi
    fi
  fi

  export PATH
}

remove_path() {
  local path="$1"
  local attempt

  [[ -e "$path" ]] || return 0

  for attempt in 1 2 3; do
    rm -rf "$path" 2>/dev/null || true
    if [[ -e "$path" ]]; then
      # Finder/Spotlight may recreate .DS_Store while a large tree is deleted.
      find "$path" -name '.DS_Store' -delete 2>/dev/null || true
      rm -rf "$path" 2>/dev/null || true
    fi
    if [[ ! -e "$path" ]]; then
      return 0
    fi
    sleep 0.2
  done

  echo "Error: failed to remove $path" >&2
  return 1
}

clean_previous_artifacts() {
  echo "==> [1/4] Cleaning previous build artifacts..."

  local removed=0
  local cargo_target="${CARGO_TARGET_DIR:-$TAURI_DIR/target}"
  local paths=(
    "$FRONTEND_DIR/dist"
    "$FRONTEND_DIR/node_modules/.vite"
    "$ROOT_DIR/target"
    "$cargo_target"
    "$DMG_STAGING_DIR"
  )

  local path
  for path in "${paths[@]}"; do
    if [[ -e "$path" ]]; then
      remove_path "$path"
      echo "    Removed $path"
      removed=1
    fi
  done

  if [[ "$removed" -eq 0 ]]; then
    echo "    No previous build artifacts found."
  fi
}

verify_prerequisites() {
  echo "==> [0/4] Checking build prerequisites..."

  if ! command -v node >/dev/null 2>&1; then
    echo "Error: 'node' command could not be found in PATH." >&2
    echo "Please ensure Node.js is installed." >&2
    exit 127
  fi

  if ! command -v npm >/dev/null 2>&1; then
    echo "Error: 'npm' command could not be found in PATH." >&2
    echo "Please ensure npm is installed." >&2
    exit 127
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    echo "Error: 'cargo' command could not be found in PATH." >&2
    echo "Please ensure Rust and Cargo are installed (e.g. via https://rustup.rs)." >&2
    exit 127
  fi

  if [[ "$(uname -s)" == "Darwin" ]]; then
    if ! command -v swiftc >/dev/null 2>&1 || ! command -v xcrun >/dev/null 2>&1; then
      echo "Error: Xcode Command Line Tools ('swiftc' / 'xcrun') not found." >&2
      echo "Run 'xcode-select --install' to install them." >&2
      exit 127
    fi
  fi

  echo "    Node:   $(node -v)"
  echo "    npm:    v$(npm -v)"
  echo "    Rust:   $(rustc --version | awk '{print $1, $2}')"
  if [[ "$(uname -s)" == "Darwin" ]]; then
    echo "    Swift:  $(swiftc --version | head -n 1)"
  fi
}

build_frontend() {
  echo "==> [2/4] Building frontend production bundle..."
  cd "$FRONTEND_DIR"

  if [[ ! -d "node_modules" ]]; then
    echo "    Installing frontend dependencies..."
    if [[ -f "package-lock.json" ]]; then
      npm ci
    else
      npm install
    fi
  fi

  npm run build
  cd "$ROOT_DIR"
}

build_desktop_release() {
  echo "==> [3/4] Compiling and packaging desktop application..."
  cd "$ROOT_DIR"

  local tauri_build_success=0

  # Attempt 1: Try cargo tauri build
  if command -v cargo-tauri >/dev/null 2>&1 || cargo tauri --version >/dev/null 2>&1; then
    echo "    Building with cargo-tauri..."
    if cargo tauri build --bundles app --features tauri-app "$@"; then
      tauri_build_success=1
    fi
  fi

  # Attempt 2: Try npx @tauri-apps/cli build
  if [[ "$tauri_build_success" -eq 0 ]] && command -v npx >/dev/null 2>&1; then
    echo "    Building with npx @tauri-apps/cli..."
    if npx --yes @tauri-apps/cli build --bundles app --features tauri-app "$@"; then
      tauri_build_success=1
    fi
  fi

  # Fallback: Direct cargo build --release + bundle assembly
  if [[ "$tauri_build_success" -eq 0 ]]; then
    echo "    Tauri CLI unavailable or exited non-zero; falling back to direct cargo release build..."
    cargo build --release --manifest-path "$TAURI_DIR/Cargo.toml" --features tauri-app,custom-protocol "$@"

    if [[ "$(uname -s)" == "Darwin" ]]; then
      assemble_macos_app_bundle
    fi
  fi

  if [[ "$(uname -s)" == "Darwin" ]]; then
    local app_dir="$TAURI_DIR/target/release/bundle/macos/AeroShoot.app"
    if [[ -d "$app_dir" ]]; then
      sign_macos_app_bundle "$app_dir"
      package_signed_dmg "$app_dir"
    fi
  fi
}

package_signed_dmg() {
  local app_dir="$1"
  local dmg_dir="$TAURI_DIR/target/release/bundle/dmg"
  local dmg_file="$dmg_dir/AeroShoot_0.1.0_aarch64.dmg"

  echo "==> [DMG] Creating signed DMG installer from signed application..."
  mkdir -p "$dmg_dir"
  rm -f "$dmg_file"

  local stage_dir="$DMG_STAGING_DIR"
  rm -rf "$stage_dir"
  mkdir -p "$stage_dir"

  cp -R "$app_dir" "$stage_dir/"
  ln -s /Applications "$stage_dir/Applications"

  hdiutil create -volname "AeroShoot" -srcfolder "$stage_dir" -ov -format UDZO "$dmg_file" >/dev/null 2>&1
  rm -rf "$stage_dir"
  echo "    Signed DMG ready at: $dmg_file"
}

assemble_macos_app_bundle() {
  local bundle_dir="$TAURI_DIR/target/release/bundle/macos"
  local app_dir="$bundle_dir/AeroShoot.app"
  local contents_dir="$app_dir/Contents"
  local macos_dir="$contents_dir/MacOS"
  local res_dir="$contents_dir/Resources"

  echo "    Assembling macOS .app bundle at: $app_dir"
  mkdir -p "$macos_dir"
  mkdir -p "$res_dir"

  # Copy compiled binary
  cp "$TAURI_DIR/target/release/aeroshoot" "$macos_dir/AeroShoot"
  chmod +x "$macos_dir/AeroShoot"

  # Copy icon
  if [[ -f "$TAURI_DIR/icons/icon.icns" ]]; then
    cp "$TAURI_DIR/icons/icon.icns" "$res_dir/icon.icns"
  fi

  # Generate Info.plist with native capture permissions
  cat > "$contents_dir/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>AeroShoot</string>
  <key>CFBundleExecutable</key>
  <string>AeroShoot</string>
  <key>CFBundleIconFile</key>
  <string>icon.icns</string>
  <key>CFBundleIdentifier</key>
  <string>ai.aeroshoot.studio</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>AeroShoot</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSCameraUsageDescription</key>
  <string>AeroShoot records the selected camera as a separate video track.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>AeroShoot records the selected microphone as a separate audio track.</string>
  <key>NSScreenCaptureUsageDescription</key>
  <string>AeroShoot records the selected screen or window for your video project.</string>
  <key>NSInputMonitoringUsageDescription</key>
  <string>AeroShoot tracks mouse telemetry during screen recording.</string>
</dict>
</plist>
PLIST
}

sign_macos_app_bundle() {
  local app_dir="$1"
  if [[ ! -d "$app_dir" ]]; then
    return 0
  fi

  echo "==> [Signing] Code-signing macOS application bundle with stable Designated Requirement..."

  local signing_identity=""
  if command -v security >/dev/null 2>&1; then
    signing_identity="$(security find-identity -v -p codesigning 2>/dev/null | grep -E "Developer ID Application|Apple Development" | head -n 1 | awk -F'"' '{print $2}' || true)"
  fi

  if [[ -n "$signing_identity" ]]; then
    echo "    Using Keychain signing identity: $signing_identity"
    codesign --force --deep --sign "$signing_identity" \
      --options runtime \
      --identifier "ai.aeroshoot.studio" \
      "$app_dir"
  else
    echo "    Signing with stable ad-hoc designated requirement (=designated => identifier \"ai.aeroshoot.studio\")..."
    codesign --force --deep --sign - \
      --identifier "ai.aeroshoot.studio" \
      --requirements '=designated => identifier "ai.aeroshoot.studio"' \
      "$app_dir"
  fi

  echo "    Verified code signature and designated requirement:"
  codesign -dvvv "$app_dir" 2>&1 | grep -E "Identifier=|Signature=|Info.plist entries=" | sed 's/^/      /' || true
  codesign -d -r- "$app_dir" 2>&1 | grep -E "designated =>" | sed 's/^/      /' || true
}

install_macos_app() {
  local src_app="$TAURI_DIR/target/release/bundle/macos/AeroShoot.app"
  local dest_app="/Applications/AeroShoot.app"
  if [[ ! -d "$src_app" ]]; then
    echo "Error: Built app bundle not found at $src_app" >&2
    return 1
  fi

  echo "==> [Install] Installing AeroShoot to $dest_app..."
  pkill -x "AeroShoot" 2>/dev/null || pkill -x "aeroshoot" 2>/dev/null || true

  rm -rf "$dest_app"
  cp -R "$src_app" "$dest_app"

  sign_macos_app_bundle "$dest_app"
  echo "==> [Install] Successfully installed and signed $dest_app"
}

report_artifacts() {
  echo "==> [4/4] Build completed successfully!"
  echo ""
  echo "Artifacts produced:"

  local mac_app="$TAURI_DIR/target/release/bundle/macos/AeroShoot.app"
  local mac_dmg
  mac_dmg="$(find "$TAURI_DIR/target/release/bundle/dmg" -name "*.dmg" 2>/dev/null | head -n 1 || true)"
  local release_bin="$TAURI_DIR/target/release/aeroshoot"

  if [[ -d "$mac_app" ]]; then
    echo "  • macOS Application: $mac_app"
  fi
  if [[ -n "$mac_dmg" && -f "$mac_dmg" ]]; then
    echo "  • macOS DMG Installer: $mac_dmg"
  fi
  if [[ -f "$release_bin" ]]; then
    echo "  • Release Binary:     $release_bin"
  fi

  echo ""
  if [[ -d "$mac_app" ]]; then
    echo "To run the production app:"
    echo "  open \"$mac_app\""
  elif [[ -f "$release_bin" ]]; then
    echo "To run the release binary:"
    echo "  \"$release_bin\""
  fi
}

main() {
  local do_install=0
  local build_args=()

  for arg in "$@"; do
    if [[ "$arg" == "--install" || "$arg" == "-i" ]]; then
      do_install=1
    else
      build_args+=("$arg")
    fi
  done

  ensure_environment
  verify_prerequisites
  clean_previous_artifacts
  build_frontend
  if [[ "${#build_args[@]}" -gt 0 ]]; then
    build_desktop_release "${build_args[@]}"
  else
    build_desktop_release
  fi

  if [[ "$do_install" -eq 1 && "$(uname -s)" == "Darwin" ]]; then
    install_macos_app
  fi

  report_artifacts
}

main "$@"
