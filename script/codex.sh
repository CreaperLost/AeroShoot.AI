#!/usr/bin/env bash
set -euo pipefail

MODE="${1:-help}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FRONTEND_DIR="$ROOT_DIR/front-end"
TAURI_DIR="$ROOT_DIR/src-tauri"
DEV_PORT="${AEROSHOOT_DEV_PORT:-1420}"
# Processes this launcher started, one "<pid> <start time>" line each. The
# start time guards against acting on a reused PID.
RUN_DIR="$ROOT_DIR/.codex/run"
DEV_PID_FILE="$RUN_DIR/dev-server.pid"

show_usage() {
  cat <<'USAGE'
usage: ./script/codex.sh <setup|start|stop|build|install>

Commands:
  setup    Install the frontend dependencies from package-lock.json
  start    Start the Vite development server in the foreground
  stop     Stop the Vite development server started by `start`
  build    Run frontend and Rust tests, then build the production application
           (macOS: signed .app and DMG; extra arguments go to build.sh)
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

# Print the start time of a running PID, or nothing.
process_start_time() {
  ps -o lstart= -p "$1" 2>/dev/null | sed 's/^ *//' || true
}

# True when the PID file names a live process with the recorded start time.
tracked_process_alive() {
  local pid_file="$1" pid started
  [[ -f "$pid_file" ]] || return 1
  read -r pid started < "$pid_file" || return 1
  [[ "$pid" =~ ^[0-9]+$ ]] || return 1
  [[ -n "$started" && "$(process_start_time "$pid")" == "$started" ]]
}

tracked_pid() {
  local pid _
  read -r pid _ < "$1"
  printf '%s' "$pid"
}

port_in_use() {
  if command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$DEV_PORT" -sTCP:LISTEN >/dev/null 2>&1
  elif command -v ss >/dev/null 2>&1; then
    ss -Hltn "sport = :$DEV_PORT" 2>/dev/null | grep -q .
  else
    return 1
  fi
}

# Every descendant of a PID, deepest first.
descendants() {
  local child
  for child in $(pgrep -P "$1" 2>/dev/null || true); do
    descendants "$child"
    printf '%s\n' "$child"
  done
}

start() {
  ensure_environment
  require_frontend
  if [[ ! -x "$FRONTEND_DIR/node_modules/.bin/vite" ]]; then
    echo "Frontend dependencies are not installed. Run the Setup action first." >&2
    exit 1
  fi
  if tracked_process_alive "$DEV_PID_FILE"; then
    echo "The AeroShoot dev server is already running (pid $(tracked_pid "$DEV_PID_FILE")). Run Stop first." >&2
    exit 1
  fi
  rm -f "$DEV_PID_FILE"
  if port_in_use; then
    echo "Port $DEV_PORT is in use by another process; Vite needs it (strictPort)." >&2
    exit 1
  fi

  mkdir -p "$RUN_DIR"
  # `exec` keeps this PID, so the recorded process is the dev server itself.
  printf '%s %s\n' "$$" "$(process_start_time "$$")" > "$DEV_PID_FILE"
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

  if ! tracked_process_alive "$DEV_PID_FILE"; then
    rm -f "$DEV_PID_FILE"
    echo "No AeroShoot dev server started by this launcher is running."
    return 0
  fi

  local pid
  pid="$(tracked_pid "$DEV_PID_FILE")"
  # Collect the tree before signalling: children re-parent once npm exits.
  local tree
  tree="$(descendants "$pid") $pid"
  # shellcheck disable=SC2086
  kill -TERM $tree 2>/dev/null || true

  local waited=0
  while [[ "$waited" -lt 100 ]]; do
    local alive=0 member
    for member in $tree; do
      if kill -0 "$member" 2>/dev/null; then alive=1; fi
    done
    [[ "$alive" -eq 0 ]] && break
    sleep 0.1
    waited=$((waited + 1))
  done
  # shellcheck disable=SC2086
  kill -KILL $tree 2>/dev/null || true
  rm -f "$DEV_PID_FILE"

  # Start needs the port back (strictPort); wait briefly for the socket.
  waited=0
  while port_in_use && [[ "$waited" -lt 50 ]]; do
    sleep 0.1
    waited=$((waited + 1))
  done
  echo "Stopped AeroShoot dev server (pid $pid)."
}

validate() {
  ensure_environment
  require_frontend
  echo "==> Validating: frontend tests"
  (
    cd "$FRONTEND_DIR"
    if [[ ! -d node_modules ]]; then
      npm ci
    fi
    npm test
  )
  echo "==> Validating: Rust unit and integration tests"
  cargo test --manifest-path "$TAURI_DIR/Cargo.toml" --no-default-features --lib --tests
}

build() {
  validate
  # The DMG is macOS-only; build.sh rejects --dmg elsewhere.
  if [[ "$#" -eq 0 && "$(uname -s)" == "Darwin" ]]; then
    set -- --dmg
  fi
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
