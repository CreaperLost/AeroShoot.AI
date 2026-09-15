#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-help}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$ROOT_DIR/front-end"
DEV_PORT="${AEROSHOOT_DEV_PORT:-1420}"

show_usage() {
  cat <<'USAGE'
usage: ./script/codex.sh <setup|start|stop|build|install>

Commands:
  setup    Install the frontend dependencies from package-lock.json
  start    Start the Vite development server in the foreground
  stop     Stop this project's Vite server listening on port 1420
  build    Build the signed production application (pass --dmg to also create a DMG)
  install  Rebuild, verify, and replace /Applications/AeroShoot.app
USAGE
}

ensure_environment() {
  # 1. Ensure basic system directories are in PATH
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
  if ! command -v npm >/dev/null 2>&1 || ! command -v node >/dev/null 2>&1; then
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

  # 6. Verify required binaries are available
  if ! command -v npm >/dev/null 2>&1; then
    echo "Error: 'npm' command could not be found in PATH." >&2
    echo "Current PATH: $PATH" >&2
    echo "Please ensure Node.js and npm are installed and accessible." >&2
    exit 127
  fi

  if ! command -v node >/dev/null 2>&1; then
    echo "Error: 'node' command could not be found in PATH." >&2
    echo "Current PATH: $PATH" >&2
    echo "Please ensure Node.js is installed and accessible." >&2
    exit 127
  fi
}

require_frontend() {
  if [[ ! -f "$FRONTEND_DIR/package.json" ]]; then
    echo "Frontend package.json not found at $FRONTEND_DIR" >&2
    exit 1
  fi
}

setup() {
  ensure_environment
  require_frontend
  cd "$FRONTEND_DIR"

  if [[ -f package-lock.json ]]; then
    npm ci
  else
    npm install
  fi
}

start() {
  ensure_environment
  require_frontend
  if [[ ! -x "$FRONTEND_DIR/node_modules/.bin/vite" ]]; then
    echo "Frontend dependencies are not installed. Run the Setup action first." >&2
    exit 1
  fi

  cd "$FRONTEND_DIR"
  exec npm run dev
}

stop() {
  for sys_dir in /usr/sbin /sbin /usr/bin /bin; do
    if [[ -d "$sys_dir" && ":$PATH:" != *":$sys_dir:"* ]]; then
      PATH="$PATH:$sys_dir"
    fi
  done
  export PATH

  local stopped=0
  local pid
  local process_command

  while read -r pid; do
    [[ -n "$pid" ]] || continue
    process_command="$(ps -o command= -p "$pid" 2>/dev/null || true)"

    if [[ "$process_command" == *"$FRONTEND_DIR"* ]]; then
      kill "$pid" 2>/dev/null || true
      stopped=1
      echo "Stopped AeroShoot Vite server (pid $pid)."
    fi
  done < <(lsof -tiTCP:"$DEV_PORT" -sTCP:LISTEN 2>/dev/null || true)

  if [[ "$stopped" -eq 0 ]]; then
    echo "No AeroShoot Vite server is listening on port $DEV_PORT."
  fi
}

build() {
  exec bash "$ROOT_DIR/script/build.sh" "$@"
}

case "$MODE" in
  setup)
    setup
    ;;
  start)
    start
    ;;
  stop)
    stop
    ;;
  build)
    shift || true
    build "$@"
    ;;
  install)
    shift || true
    exec bash "$ROOT_DIR/script/build.sh" --install "$@"
    ;;
  --help|help)
    show_usage
    ;;
  *)
    show_usage >&2
    exit 2
    ;;
esac
