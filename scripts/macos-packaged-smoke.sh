#!/usr/bin/env bash
# Unsigned macOS .app lifecycle smoke.
#
# The app must be built with src-tauri/tauri.smoke.conf.json so its bundle ID,
# WebKit data, settings, operation lock, and logs are isolated from a real user
# install. No signing or notarization credentials are read by this script.

set -euo pipefail

usage() {
  echo "usage: $0 APP --expected-arch ARCH --expected-lang LANG --apple-language TAG" >&2
  exit 2
}

[[ $# -ge 1 ]] || usage
APP=$1
shift

EXPECTED_ARCH=""
EXPECTED_LANG=""
APPLE_LANGUAGE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --expected-arch)
      [[ $# -ge 2 ]] || usage
      EXPECTED_ARCH=$2
      shift 2
      ;;
    --expected-lang)
      [[ $# -ge 2 ]] || usage
      EXPECTED_LANG=$2
      shift 2
      ;;
    --apple-language)
      [[ $# -ge 2 ]] || usage
      APPLE_LANGUAGE=$2
      shift 2
      ;;
    *) usage ;;
  esac
done
[[ -n "$EXPECTED_ARCH" && -n "$EXPECTED_LANG" && -n "$APPLE_LANGUAGE" ]] || usage

stage() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::group::[$1] $2"
  else
    echo "[$1] $2"
  fi
}

end_stage() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::endgroup::"
  fi
}

fail() {
  if [[ "${GITHUB_ACTIONS:-}" == "true" ]]; then
    echo "::error::[$1] $2" >&2
  else
    echo "[$1] ERROR: $2" >&2
  fi
  exit 1
}

plist() {
  /usr/libexec/PlistBuddy -c "Print :$2" "$1/Contents/Info.plist"
}

menu_copy() {
  case "$EXPECTED_LANG" in
    en) EDIT_MENU="Edit"; WINDOW_MENU="Window"; QUIT_ITEM="Quit Codex App Manager" ;;
    zh-CN) EDIT_MENU="编辑"; WINDOW_MENU="窗口"; QUIT_ITEM="退出 Codex App Manager" ;;
    zh-TW) EDIT_MENU="編輯"; WINDOW_MENU="視窗"; QUIT_ITEM="結束 Codex App Manager" ;;
    ja) EDIT_MENU="編集"; WINDOW_MENU="ウインドウ"; QUIT_ITEM="Codex App Managerを終了" ;;
    ko) EDIT_MENU="편집"; WINDOW_MENU="윈도우"; QUIT_ITEM="Codex App Manager 종료" ;;
    fr) EDIT_MENU="Édition"; WINDOW_MENU="Fenêtre"; QUIT_ITEM="Quitter Codex App Manager" ;;
    de) EDIT_MENU="Bearbeiten"; WINDOW_MENU="Fenster"; QUIT_ITEM="Codex App Manager beenden" ;;
    es) EDIT_MENU="Edición"; WINDOW_MENU="Ventana"; QUIT_ITEM="Salir de Codex App Manager" ;;
    pt-BR) EDIT_MENU="Editar"; WINDOW_MENU="Janela"; QUIT_ITEM="Encerrar Codex App Manager" ;;
    ru) EDIT_MENU="Правка"; WINDOW_MENU="Окно"; QUIT_ITEM="Завершить Codex App Manager" ;;
    ar) EDIT_MENU="تحرير"; WINDOW_MENU="نافذة"; QUIT_ITEM="إنهاء Codex App Manager" ;;
    *) fail "locale" "unsupported expected language: $EXPECTED_LANG" ;;
  esac
}

exact_pids() {
  pgrep -f "$BINARY" 2>/dev/null || true
}

process_count() {
  local pids
  pids=$(exact_pids)
  if [[ -z "$pids" ]]; then
    echo 0
  else
    echo "$pids" | wc -l | tr -d ' '
  fi
}

wait_for_process() {
  local deadline=$((SECONDS + 20))
  while (( SECONDS < deadline )); do
    local pids
    pids=$(exact_pids)
    if [[ -n "$pids" ]]; then
      echo "$pids" | head -1
      return 0
    fi
    sleep 0.25
  done
  return 1
}

log_since_run() {
  [[ -f "$LOG_FILE" ]] || return 0
  awk -v marker="packaged smoke run id=$RUN_ID" '
    index($0, marker) { seen = 1 }
    seen { print }
  ' "$LOG_FILE"
}

wait_for_log() {
  local needle=$1
  local deadline=$((SECONDS + 25))
  while (( SECONDS < deadline )); do
    if log_since_run | grep -Fq "$needle"; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_for_log_count() {
  local needle=$1
  local expected=$2
  local deadline=$((SECONDS + 25))
  while (( SECONDS < deadline )); do
    local count
    count=$(log_since_run | grep -Fc "$needle" || true)
    if (( count >= expected )); then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

wait_for_absence() {
  local path=$1
  local deadline=$((SECONDS + 15))
  while (( SECONDS < deadline )); do
    if [[ ! -e "$path" && ! -L "$path" ]]; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

send_command_shortcut() {
  local pid=$1
  local key=$2
  osascript - "$pid" "$key" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  set keyName to item 2 of argv
  tell application "System Events"
    tell first application process whose unix id is targetPid
      set frontmost to true
      delay 0.5
      keystroke keyName using command down
    end tell
  end tell
end run
APPLESCRIPT
}

send_escape() {
  local pid=$1
  osascript - "$pid" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  tell application "System Events"
    tell first application process whose unix id is targetPid
      set frontmost to true
      delay 0.5
      key code 53
    end tell
  end tell
end run
APPLESCRIPT
}

window_minimized() {
  local pid=$1
  osascript - "$pid" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  tell application "System Events"
    tell first application process whose unix id is targetPid
      return value of attribute "AXMinimized" of window 1
    end tell
  end tell
end run
APPLESCRIPT
}

process_visible() {
  local pid=$1
  osascript - "$pid" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  tell application "System Events"
    return visible of first application process whose unix id is targetPid
  end tell
end run
APPLESCRIPT
}

assert_native_menu() {
  local pid=$1
  osascript - "$pid" "Codex App Manager" "$EDIT_MENU" "$WINDOW_MENU" "$QUIT_ITEM" <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  set productName to item 2 of argv
  set editName to item 3 of argv
  set windowName to item 4 of argv
  set quitName to item 5 of argv
  tell application "System Events"
    tell first application process whose unix id is targetPid
      if not (exists menu bar item productName of menu bar 1) then error "missing app menu: " & productName
      if not (exists menu bar item editName of menu bar 1) then error "missing Edit menu: " & editName
      if not (exists menu bar item windowName of menu bar 1) then error "missing Window menu: " & windowName
      if not (exists menu item quitName of menu 1 of menu bar item productName of menu bar 1) then error "missing Quit item: " & quitName
    end tell
  end tell
end run
APPLESCRIPT
}

click_quit_menu() {
  local pid=$1
  osascript - "$pid" "Codex App Manager" "$QUIT_ITEM" >/dev/null <<'APPLESCRIPT'
on run argv
  set targetPid to (item 1 of argv) as integer
  set productName to item 2 of argv
  set quitName to item 3 of argv
  tell application "System Events"
    tell first application process whose unix id is targetPid
      set frontmost to true
      delay 0.5
      click menu item quitName of menu 1 of menu bar item productName of menu bar 1
    end tell
  end tell
end run
APPLESCRIPT
}

wait_for_window_state() {
  local pid=$1
  local probe=$2
  local expected=$3
  local deadline=$((SECONDS + 12))
  while (( SECONDS < deadline )); do
    local actual=""
    if [[ "$probe" == "minimized" ]]; then
      actual=$(window_minimized "$pid" 2>/dev/null || true)
    else
      actual=$(process_visible "$pid" 2>/dev/null || true)
    fi
    if [[ "$actual" == "$expected" ]]; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

launch_app() {
  local posix_locale
  case "$EXPECTED_LANG" in
    zh-CN) posix_locale="zh_CN.UTF-8" ;;
    zh-TW) posix_locale="zh_TW.UTF-8" ;;
    ja) posix_locale="ja_JP.UTF-8" ;;
    ko) posix_locale="ko_KR.UTF-8" ;;
    fr) posix_locale="fr_FR.UTF-8" ;;
    de) posix_locale="de_DE.UTF-8" ;;
    es) posix_locale="es_ES.UTF-8" ;;
    pt-BR) posix_locale="pt_BR.UTF-8" ;;
    ru) posix_locale="ru_RU.UTF-8" ;;
    ar) posix_locale="ar_SA.UTF-8" ;;
    *) posix_locale="en_US.UTF-8" ;;
  esac
  open -n -F \
    --env "CAM_PACKAGED_SMOKE_RUN=$RUN_ID" \
    --env "CAM_PACKAGED_SMOKE_DATA_DIR=$SMOKE_DATA_DIR" \
    --env "LANG=$posix_locale" \
    --env "LC_ALL=$posix_locale" \
    "$APP"
}

[[ -d "$APP" ]] || fail "bundle" "app bundle not found: $APP"
BUNDLE_ID=$(plist "$APP" CFBundleIdentifier)
[[ "$BUNDLE_ID" == "io.github.wangnov.codexappmanager.smoke" ]] ||
  fail "bundle" "refusing to smoke non-isolated bundle id: $BUNDLE_ID"
PRODUCT_NAME=$(plist "$APP" CFBundleName)
[[ "$PRODUCT_NAME" == "Codex App Manager" ]] ||
  fail "bundle" "unexpected product name: $PRODUCT_NAME"
EXECUTABLE=$(plist "$APP" CFBundleExecutable)
BINARY="$APP/Contents/MacOS/$EXECUTABLE"
[[ -x "$BINARY" ]] || fail "bundle" "main executable missing: $BINARY"
LOG_FILE="$HOME/Library/Logs/$BUNDLE_ID/codex-app-manager.log"
RUN_ID="$(date +%s)-$$-${RANDOM:-0}"
SMOKE_TEMP_ROOT=$(cd "${TMPDIR:-/tmp}" && pwd -P)
SMOKE_DATA_DIR="$SMOKE_TEMP_ROOT/codex-app-manager-smoke-$RUN_ID"
if [[ -e "$SMOKE_DATA_DIR" || -L "$SMOKE_DATA_DIR" ]]; then
  fail "isolation" "refusing to reuse smoke data directory: $SMOKE_DATA_DIR"
fi
mkdir -m 700 "$SMOKE_DATA_DIR"
STALE_STAGING_SENTINEL="$SMOKE_DATA_DIR/staging/update-smoke-stale"
mkdir -p "$STALE_STAGING_SENTINEL"
touch -t 200001010000 "$STALE_STAGING_SENTINEL"
PREF_BACKUP=$(mktemp "${TMPDIR:-/tmp}/cam-smoke-prefs.XXXXXX.plist")
HAD_PREFS=0
if defaults export "$BUNDLE_ID" "$PREF_BACKUP" >/dev/null 2>&1; then
  HAD_PREFS=1
fi
menu_copy

cleanup() {
  local pids
  pids=$(exact_pids)
  if [[ -n "$pids" ]]; then
    echo "$pids" | xargs kill 2>/dev/null || true
    sleep 1
    pids=$(exact_pids)
    if [[ -n "$pids" ]]; then
      echo "$pids" | xargs kill -9 2>/dev/null || true
    fi
  fi
  if (( HAD_PREFS == 1 )); then
    defaults import "$BUNDLE_ID" "$PREF_BACKUP" >/dev/null 2>&1 || true
  else
    defaults delete "$BUNDLE_ID" >/dev/null 2>&1 || true
  fi
  if [[ "$SMOKE_DATA_DIR" == "$SMOKE_TEMP_ROOT/codex-app-manager-smoke-$RUN_ID" ]]; then
    rm -rf -- "$SMOKE_DATA_DIR"
    if [[ -e "$SMOKE_DATA_DIR" || -L "$SMOKE_DATA_DIR" ]]; then
      echo "[cleanup] ERROR: isolated smoke data directory was not removed" >&2
      return 1
    fi
  fi
  rm -f "$PREF_BACKUP"
}
trap cleanup EXIT

if pgrep -x "$EXECUTABLE" >/dev/null 2>&1; then
  fail "preflight" "another $EXECUTABLE process is running; stop it before the isolated smoke"
fi

stage "bundle" "Validate isolated unsigned .app and architecture"
ARCHES=$(lipo -archs "$BINARY")
case " $ARCHES " in
  *" $EXPECTED_ARCH "*) ;;
  *) fail "bundle" "expected arch $EXPECTED_ARCH, found: $ARCHES" ;;
esac
echo "bundle=$APP"
echo "bundle_id=$BUNDLE_ID executable=$EXECUTABLE arches=$ARCHES"
if codesign --verify --deep --strict "$APP" >/dev/null 2>&1; then
  echo "signature=ad-hoc-or-present (no identity required by smoke)"
else
  echo "signature=unsigned (expected and allowed for PR smoke)"
fi
end_stage

stage "locale" "Set isolated app preference AppleLanguages=$APPLE_LANGUAGE"
defaults write "$BUNDLE_ID" AppleLanguages -array "$APPLE_LANGUAGE"
echo "expected_app_language=$EXPECTED_LANG edit=$EDIT_MENU window=$WINDOW_MENU quit=$QUIT_ITEM"
end_stage

stage "launch" "Launch packaged app and prove frontend-ready IPC/CSP handshake"
launch_app
PID=$(wait_for_process) || fail "launch" "packaged process did not start"
wait_for_log "packaged smoke run id=$RUN_ID data_dir_isolated=true" ||
  fail "launch" "isolated smoke marker missing from app log: $LOG_FILE"
[[ -f "$SMOKE_DATA_DIR/operation.lock" ]] ||
  fail "launch" "operation lock was not created in isolated data directory"
wait_for_absence "$STALE_STAGING_SENTINEL" ||
  fail "launch" "startup cleanup did not use the isolated staging directory"
wait_for_log "frontend readiness token injected generation=1" ||
  fail "launch" "backend did not bind a readiness token to the packaged document"
wait_for_log "frontend ready lang=$EXPECTED_LANG" ||
  fail "launch" "frontend-ready IPC did not report expected language $EXPECTED_LANG"
wait_for_log "native menu installed lang=$EXPECTED_LANG" ||
  fail "launch" "native menu did not switch to expected language $EXPECTED_LANG"
[[ "$(process_count)" == "1" ]] || fail "launch" "expected one main process"
echo "pid=$PID log=$LOG_FILE"
end_stage

stage "menu" "Verify localized native menu structure and custom Quit item"
assert_native_menu "$PID" || fail "menu" "native menu inspection failed (Accessibility/UI scripting unavailable or labels incorrect)"
end_stage

stage "single-instance" "Recover the main window from minimized and hidden states"
send_command_shortcut "$PID" "m" || fail "single-instance" "Cmd+M injection failed"
wait_for_window_state "$PID" minimized true || fail "single-instance" "window did not minimize"
launch_app
wait_for_log_count "single-instance activation requested" 1 || fail "single-instance" "second launch did not reach activation callback"
wait_for_window_state "$PID" minimized false || fail "single-instance" "second launch did not unminimize the window"
[[ "$(process_count)" == "1" ]] || fail "single-instance" "second launch left more than one process"

send_command_shortcut "$PID" "h" || fail "single-instance" "Cmd+H injection failed"
wait_for_window_state "$PID" visible false || fail "single-instance" "application did not hide"
launch_app
wait_for_log_count "single-instance activation requested" 2 || fail "single-instance" "hidden-state relaunch did not reach activation callback"
wait_for_window_state "$PID" visible true || fail "single-instance" "second launch did not reveal the hidden application"
[[ "$(process_count)" == "1" ]] || fail "single-instance" "hidden-state relaunch left more than one process"
end_stage

stage "close" "Exercise Cmd+W close interception after frontend readiness"
send_command_shortcut "$PID" "w" || fail "close" "Cmd+W injection failed"
wait_for_log "window close requested label=main" || fail "close" "window CloseRequested was not observed"
wait_for_log_count "shell event emitted kind=confirm-quit" 1 || fail "close" "close confirmation event was not delivered"
kill -0 "$PID" 2>/dev/null || fail "close" "process exited instead of waiting for close confirmation"
send_escape "$PID" || fail "close" "could not dismiss close confirmation"
end_stage

stage "quit" "Exercise Cmd+Q accelerator and the native menu Quit item"
send_command_shortcut "$PID" "q" || fail "quit" "Cmd+Q injection failed"
wait_for_log_count "menu quit requested id=cam-quit" 1 || fail "quit" "Cmd+Q did not route through custom menu handler"
wait_for_log_count "shell event emitted kind=confirm-quit" 2 || fail "quit" "Cmd+Q did not deliver a confirmation event"
kill -0 "$PID" 2>/dev/null || fail "quit" "Cmd+Q bypassed the confirmation guard"
send_escape "$PID" || fail "quit" "could not dismiss Cmd+Q confirmation"

click_quit_menu "$PID" || fail "quit" "native Quit menu click failed"
wait_for_log_count "menu quit requested id=cam-quit" 2 || fail "quit" "menu click did not route through custom menu handler"
wait_for_log_count "shell event emitted kind=confirm-quit" 3 || fail "quit" "menu Quit did not deliver a confirmation event"
kill -0 "$PID" 2>/dev/null || fail "quit" "menu Quit bypassed the confirmation guard"
end_stage

echo "macOS packaged smoke passed: bundle → locale/menu → IPC/CSP → single-instance restore → close → Cmd+Q/menu Quit"
