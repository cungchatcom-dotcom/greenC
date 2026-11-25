#!/usr/bin/env bash
# GreenC Headless Disk Guardian for Linux (thin cron/@reboot wrapper)
#
# Delegates quota evaluation, atomic developer-cache cleanup, same-drive Vault net accounting,
# and persistent DB cooldowns (30m -> 2h -> 12h) to `greenc-cli guard run --json`.
# Keeps the 2026-11-24 shell process check as defense-in-depth via `--extra-busy-tools`,
# avoiding `ps | awk` SIGPIPE hazards and guaranteeing that early errors never suppress logging.
#
# Usage:
#   greenc-disk-guard.sh [--dry-run] [--reboot] [--drive /home]
#
# Environment overrides (optional, also used by automated tests):
#   GREENC_CLI            Path to greenc-cli binary (default: ~/.local/bin/greenc-cli)
#   GREENC_LOG_DIR        Directory for guardian.log (default: ~/.local/share/greenc)
#   GREENC_LOG_FILE       Log file path (default: $GREENC_LOG_DIR/guardian.log)
#   GREENC_LOG_MAX_BYTES  Max log size before rotating to .1 (default: 1048576 = 1 MiB)
#   GREENC_LOCK_FILE      flock path (default: /tmp/greenc-disk-guard-<uid>.lock)

# Intentionally omit `-e`: early non-fatal steps must never abort the script before logging.
set -uo pipefail

CURRENT_STEP="init"
LOGGED_RESULT=0
ERR_TMP=""

if [[ -z "${HOME:-}" ]]; then
  passwd_line="$(getent passwd "$(id -u)" 2>/dev/null || true)"
  if [[ -n "$passwd_line" ]]; then
    IFS=: read -r _ _ _ _ _ HOME _ <<< "$passwd_line"
  fi
  HOME="${HOME:-/root}"
  export HOME
fi

export PATH="${HOME}/.local/bin:/usr/local/bin:/usr/bin:/bin:${PATH:-}"

GREENC_CLI="${GREENC_CLI:-${HOME}/.local/bin/greenc-cli}"
LOG_DIR="${GREENC_LOG_DIR:-${HOME}/.local/share/greenc}"
LOG_FILE="${GREENC_LOG_FILE:-${LOG_DIR}/guardian.log}"
LOG_MAX_BYTES="${GREENC_LOG_MAX_BYTES:-1048576}"
LOCK_FILE="${GREENC_LOCK_FILE:-/tmp/greenc-disk-guard-$(id -u).lock}"

DRY_RUN=0
IS_REBOOT=0
TARGET_DRIVE=""
ARG_ERR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --reboot)
      IS_REBOOT=1
      shift
      ;;
    --drive)
      if [[ $# -lt 2 ]]; then
        ARG_ERR="--drive requires a mount point argument"
        break
      fi
      TARGET_DRIVE="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

ts() {
  date '+%Y-%m-%d %H:%M:%S %z'
}

rotate_log_if_needed() {
  if [[ -f "$LOG_FILE" ]]; then
    local size
    size="$(stat -c%s "$LOG_FILE" 2>/dev/null || wc -c <"$LOG_FILE" 2>/dev/null || echo 0)"
    if [[ "$size" -ge "$LOG_MAX_BYTES" ]]; then
      mv -f "$LOG_FILE" "${LOG_FILE}.1" 2>/dev/null || true
    fi
  fi
}

log() {
  rotate_log_if_needed
  local line
  line="[$(ts)] $*"
  printf '%s\n' "$line" >> "$LOG_FILE" 2>/dev/null || printf '%s\n' "$line" >&2
  if [[ -t 1 ]]; then
    printf '%s\n' "$line" || true
  fi
}

if ! mkdir -p "$LOG_DIR" 2>/dev/null; then
  printf '[%s] ERROR: failed to create log dir %q (reboot=%s)\n' "$(ts)" "$LOG_DIR" "$IS_REBOOT" >&2
  exit 1
fi

cleanup_and_ensure_log() {
  local rc=$?
  if [[ -n "${ERR_TMP:-}" ]]; then
    rm -f "$ERR_TMP" 2>/dev/null || true
  fi
  if [[ "${LOGGED_RESULT:-0}" -eq 0 ]]; then
    log "ERROR: greenc-disk-guard exited early (exit=$rc, reboot=$IS_REBOOT, step=$CURRENT_STEP)" || true
  fi
}
trap cleanup_and_ensure_log EXIT

if [[ -n "$ARG_ERR" ]]; then
  log "ERROR: $ARG_ERR"
  LOGGED_RESULT=1
  exit 2
fi

CURRENT_STEP="flock"
if command -v flock >/dev/null 2>&1; then
  if { exec 9>"$LOCK_FILE"; } 2>/dev/null; then
    if ! flock -n 9; then
      if [[ "$IS_REBOOT" -eq 1 ]]; then
        log "REBOOT: skipped because another greenc-disk-guard instance holds $LOCK_FILE"
      else
        log "SKIP: another greenc-disk-guard instance holds $LOCK_FILE"
      fi
      LOGGED_RESULT=1
      exit 0
    fi
  else
    log "WARN: cannot open lock file $LOCK_FILE (continuing without flock)"
  fi
else
  log "WARN: flock not found in PATH (continuing without lock)"
fi

if [[ "$IS_REBOOT" -eq 1 ]]; then
  log "REBOOT: greenc-disk-guard started (pid=$$, cli=$GREENC_CLI)"
fi

CURRENT_STEP="check_cli"
if [[ ! -x "$GREENC_CLI" ]]; then
  log "ERROR: greenc-cli not found or not executable at $GREENC_CLI"
  LOGGED_RESULT=1
  exit 1
fi

# 1. Best-effort purge of expired Vault archives (never aborts or suppresses the guard pass).
CURRENT_STEP="vault_purge"
purge_json="$("$GREENC_CLI" vault purge --expired --json 2>/dev/null || echo '{"purged_archives":0,"freed_bytes":0}')"
if command -v python3 >/dev/null 2>&1; then
  purged_summary="$(python3 - "$purge_json" <<'PY' 2>/dev/null || true
import json, sys
try:
    d = json.loads(sys.argv[1])
    cnt = int(d.get("purged_archives", 0))
    freed = int(d.get("freed_bytes", 0))
    if cnt > 0:
        print(f"purged {cnt} expired archive(s) ({freed / (1024*1024):.1f} MiB)")
except Exception:
    pass
PY
)"
  if [[ -n "$purged_summary" ]]; then
    log "VAULT: $purged_summary"
  fi
fi

# 2. Defense-in-depth hotfix (2026-11-24): inspect current user's processes in shell too,
# and pass any busy dev tools to `guard run --extra-busy-tools` alongside Rust's /proc detector.
# Capture `ps` output into a variable FIRST (no `ps | awk` pipe) so early exit in `awk` can
# NEVER trigger SIGPIPE (exit 141) under `pipefail`.
CURRENT_STEP="ps_check"
ps_out=""
ps_rc=0
ps_out="$(ps -u "$(id -u)" -o pid=,comm=,args= 2>/dev/null)" || ps_rc=$?
shell_busy=""
if [[ "$ps_rc" -ne 0 ]]; then
  log "WARN: ps process check failed (exit=$ps_rc); relying on greenc-cli /proc detector"
elif [[ -n "$ps_out" ]]; then
  shell_busy="$(awk -v self_pid="$$" '
    $1 == self_pid { next }
    $2 ~ /^(grep|egrep|fgrep|rg|awk|gawk|mawk|sed|ps)$/ { next }
    $2 ~ /^(cargo|rustc|rustdoc|clippy-driver)$/ { tools["cargo"]=1 }
    $2 ~ /^(gradle|gradlew)$/ || ($2 == "java" && /GradleDaemon|GradleWrapperMain|GradleWorkerMain|KotlinCompileDaemon/) { tools["gradle"]=1 }
    $2 ~ /^(npm|pnpm|yarn)$/ || ($2 ~ /^node/ && /npm-cli\.js/) { tools["npm"]=1 }
    $2 ~ /^(pip|pip3|uv)$/ || ($2 ~ /^python/ && /(^|[[:space:]])-m[[:space:]]+(pip|pip3|uv)([[:space:]]|$)/) { tools["pip"]=1 }
    END {
      out=""
      for (t in tools) { out = (out == "" ? t : out "," t) }
      print out
    }
  ' <<< "$ps_out" || true)"
fi

# 3. Delegate quota enforcement + net accounting + DB cooldown to `greenc-cli guard run --json`.
CURRENT_STEP="guard_run"
guard_args=(guard run --json)
if [[ "$DRY_RUN" -eq 1 ]]; then
  guard_args+=(--dry-run)
fi
if [[ -n "$TARGET_DRIVE" ]]; then
  guard_args+=(--drive "$TARGET_DRIVE")
fi
if [[ -n "$shell_busy" ]]; then
  guard_args+=(--extra-busy-tools "$shell_busy")
fi

ERR_TMP="$(mktemp "${TMPDIR:-/tmp}/greenc-guard-err.XXXXXX" 2>/dev/null || mktemp "${LOG_DIR}/greenc-guard-err.XXXXXX" 2>/dev/null || true)"

guard_rc=0
if [[ -n "$ERR_TMP" ]]; then
  guard_out="$("$GREENC_CLI" "${guard_args[@]}" 2>"$ERR_TMP")" || guard_rc=$?
  guard_err="$(cat "$ERR_TMP" 2>/dev/null || true)"
else
  guard_out="$("$GREENC_CLI" "${guard_args[@]}" 2>&1)" || guard_rc=$?
  guard_err=""
fi

if [[ "$guard_rc" -ne 0 ]]; then
  log "ERROR: greenc-cli guard run failed (exit=$guard_rc): ${guard_err:-$guard_out}"
  LOGGED_RESULT=1
  exit "$guard_rc"
fi

CURRENT_STEP="log_result"
if command -v python3 >/dev/null 2>&1; then
  formatted="$(python3 - "$guard_out" <<'PY' 2>/dev/null || true
import json, sys
d = json.loads(sys.argv[1])
drives = d.get("drives", [])
if not drives:
    print("CHECK: no enabled drive quotas")
else:
    parts = []
    for drv in drives:
        root = drv.get("drive_root", "?")
        pct = float(drv.get("usage_after_ratio", 0.0)) * 100.0
        st = drv.get("status", "?")
        logical_mb = float(drv.get("logical_bytes", 0)) / (1024 * 1024)
        vault_mb = float(drv.get("vault_bytes", 0)) / (1024 * 1024)
        net_mb = float(drv.get("net_freed_bytes", 0)) / (1024 * 1024)
        reached = bool(drv.get("target_reached", False))
        seg = f"{root}={pct:.1f}% status={st} logical={logical_mb:.1f}MiB vault={vault_mb:.1f}MiB net={net_mb:+.1f}MiB target={reached}"
        reasons = drv.get("reasons") or []
        if reasons:
            seg += " [" + "; ".join(reasons) + "]"
        parts.append(seg)
    print("GUARD: " + " | ".join(parts))
PY
)"
  log "${formatted:-GUARD: $guard_out}"
else
  log "GUARD: $guard_out"
fi
LOGGED_RESULT=1

exit 0
