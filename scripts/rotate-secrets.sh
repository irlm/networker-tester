#!/usr/bin/env bash
# ==============================================================================
# rotate-secrets.sh — rotate one operational secret, and record that it happened
#
#   ./rotate-secrets.sh --list
#   ./rotate-secrets.sh --rotate jwt-secret
#   ./rotate-secrets.sh --rotate storage-key --yes
#
# DESIGN
# ------
# Rotation lives here, in an operator-run script, and NOT behind a button in the
# control plane. The control plane is internet-facing, so an endpoint that can
# rotate secrets converts any single compromise — XSS, an auth bypass, a stolen
# operator token — into total credential compromise. Rotation happens quarterly
# or on an incident; the convenience is worth very little against that. The UI
# gets the half that is safe to expose: age and status, never values
# (GET /api/admin/secrets, fed by the secret_rotation table this script writes).
#
# ONE SECRET AT A TIME, on purpose. There is no --all: the secrets here differ in
# blast radius (a JWT rotation signs everyone out; a database password needs a
# restart), and a single flag that does all of them invites running it without
# reading what it will do.
#
# THE CREDENTIAL KEY IS NOT ROTATABLE HERE — and that is the most important line
# in this file. DASHBOARD_CREDENTIAL_KEY is a DATA-ENCRYPTION key: it encrypts
# stored cloud credentials at rest. Replacing it the way you would replace a
# password does not "rotate" anything, it makes every stored credential
# permanently unreadable. It needs decrypt-with-old / re-encrypt-with-new inside
# a transaction, which is application work, not shell work. The script refuses
# and points at the procedure.
#
# Env:
#   ROTATE_ENV_FILE   /etc/alethedash-cs.env
#   ROTATE_SERVICE    alethedash-cs
#   ROTATE_STORAGE_ACCOUNT / ROTATE_STORAGE_RG   (for storage-key)
# ==============================================================================
set -euo pipefail

ENV_FILE="${ROTATE_ENV_FILE:-/etc/alethedash-cs.env}"
SERVICE="${ROTATE_SERVICE:-alethedash-cs}"
STORAGE_ACCOUNT="${ROTATE_STORAGE_ACCOUNT:-alethedashbackups}"
STORAGE_RG="${ROTATE_STORAGE_RG:-alethedash-rg}"
ASSUME_YES=0
ACTION=""
TARGET=""

log()  { echo "[$(date -u +%H:%M:%SZ)] $*"; }
die()  { echo "ROTATION FAILED: $*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
rotate-secrets.sh --list
rotate-secrets.sh --rotate <key> [--yes]

  keys:  jwt-secret  db-password  github-token  storage-key
         credential-key  (NOT automated — see docs/secret-rotation.md)
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --list)   ACTION=list ;;
        --rotate) ACTION=rotate; TARGET="${2:-}"; shift ;;
        --yes|-y) ASSUME_YES=1 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument '$1' (try --help)" ;;
    esac
    shift
done
[ -n "$ACTION" ] || { usage; exit 1; }

# ── Database handle, derived from the live service config ────────────────────
# Same rule as backup-daily.sh: never hardcode the database, read it from what
# the service is actually using.
[ -r "$ENV_FILE" ] || die "cannot read $ENV_FILE"
CONN="$(sed -n 's/^DASHBOARD_DB_URL_NPGSQL=//p' "$ENV_FILE" | head -1)"
CONN="${CONN%\"}"; CONN="${CONN#\"}"
[ -n "$CONN" ] || die "DASHBOARD_DB_URL_NPGSQL not found in $ENV_FILE"
field() { printf '%s' "$CONN" | tr ';' '\n' | sed -n "s/^ *$1 *= *//Ip" | head -1; }
DB="$(field 'database')"; DB_HOST="$(field 'host')"
DB_USER="$(field 'username')"; [ -n "$DB_USER" ] || DB_USER="$(field 'user id')"
DB_PASS="$(field 'password')"

psql_db() { PGPASSWORD="$DB_PASS" psql -h "${DB_HOST:-127.0.0.1}" -U "$DB_USER" -d "$DB" -v ON_ERROR_STOP=1 "$@"; }

record() { # <key> <note>
    local key="$1" note="$2" who
    who="$(printf '%s@%s' "${SUDO_USER:-${USER:-operator}}" "$(hostname -s)")"
    psql_db -q -c "INSERT INTO secret_rotation (secret_key, rotated_at, rotated_by, note)
                   VALUES ('${key}', now(), '${who}', '${note}')
                   ON CONFLICT (secret_key) DO UPDATE
                     SET rotated_at = EXCLUDED.rotated_at,
                         rotated_by = EXCLUDED.rotated_by,
                         note       = EXCLUDED.note;" \
        && log "recorded rotation of '${key}' by ${who}"
}

confirm() { # <prompt>
    [ "$ASSUME_YES" -eq 1 ] && return 0
    printf '%s [type yes to continue]: ' "$1"
    local reply; read -r reply </dev/tty
    [ "$reply" = "yes" ] || die "aborted by operator"
}

# Replace KEY=... in the env file, preserving mode/owner, via a temp file so a
# failed write can never leave the service with a truncated config.
set_env() { # <KEY> <value>
    local key="$1" val="$2" tmp
    tmp="$(mktemp)"; cat "$ENV_FILE" > "$tmp"
    if grep -q "^${key}=" "$tmp"; then
        grep -v "^${key}=" "$tmp" > "${tmp}.new"; mv "${tmp}.new" "$tmp"
    fi
    printf '%s=%s\n' "$key" "$val" >> "$tmp"
    cat "$tmp" > "$ENV_FILE"     # preserves the original mode/owner
    rm -f "$tmp"
}

restart_service() {
    log "restarting ${SERVICE}"
    systemctl restart "$SERVICE"
    for _ in $(seq 1 30); do
        if curl -fsS --max-time 5 http://127.0.0.1:5030/api/health >/dev/null 2>&1; then
            log "healthy"; return 0
        fi
        sleep 2
    done
    die "${SERVICE} did not become healthy after the rotation — investigate before rotating anything else"
}

# ── --list ───────────────────────────────────────────────────────────────────
if [ "$ACTION" = "list" ]; then
    psql_db -P pager=off -c \
      "SELECT secret_key, rotated_at::date AS last_rotated,
              (now()::date - rotated_at::date) AS age_days, rotated_by
         FROM secret_rotation ORDER BY rotated_at;" 2>/dev/null \
      || log "no secret_rotation table yet (pre-V055 database) — nothing recorded"
    log "full status, including secrets never rotated: GET /api/admin/secrets"
    exit 0
fi

# ── --rotate ─────────────────────────────────────────────────────────────────
case "$TARGET" in
  credential-key)
    cat >&2 <<'REFUSE'
REFUSED: credential-key is a DATA-ENCRYPTION key, not a password.

DASHBOARD_CREDENTIAL_KEY encrypts stored cloud credentials at rest. Replacing it
here would not rotate anything — it would make every stored credential
permanently unreadable, with no error at the moment of damage.

Rotating it requires decrypting with the old key and re-encrypting with the new
one in a single transaction. That is application work, not shell work.
Procedure: docs/secret-rotation.md § Credential key.
REFUSE
    exit 2 ;;

  jwt-secret)
    confirm "Rotating the session signing key will sign out every logged-in user."
    NEW="$(openssl rand -base64 32)"
    set_env DASHBOARD_JWT_SECRET "$NEW"
    restart_service
    record jwt-secret "rotated by rotate-secrets.sh; all sessions invalidated"
    log "jwt-secret rotated" ;;

  github-token)
    log "Create a new PAT, then paste it (input hidden)."
    printf 'new CANARY_GITHUB_TOKEN: '
    read -rs NEW </dev/tty; echo
    [ -n "$NEW" ] || die "empty token"
    set_env CANARY_GITHUB_TOKEN "$NEW"
    restart_service
    record github-token "rotated by rotate-secrets.sh"
    log "github-token rotated — revoke the OLD PAT in GitHub now" ;;

  db-password)
    confirm "Rotating the database password restarts the control plane."
    NEW="$(openssl rand -base64 24 | tr -d '/+=' | cut -c1-28)"
    sudo -u postgres psql -v ON_ERROR_STOP=1 -c \
        "ALTER USER \"${DB_USER}\" WITH PASSWORD '${NEW}';" >/dev/null \
        || die "ALTER USER failed — password unchanged"
    NEWCONN="$(printf '%s' "$CONN" | sed "s/Password=[^;]*/Password=${NEW}/I")"
    set_env DASHBOARD_DB_URL_NPGSQL "$NEWCONN"
    DB_PASS="$NEW"          # so the record below can still connect
    restart_service
    record db-password "rotated by rotate-secrets.sh"
    log "db-password rotated" ;;

  storage-key)
    command -v az >/dev/null 2>&1 || die "az CLI not found"
    export AZURE_CONFIG_DIR="${AZURE_CONFIG_DIR:-/root/.azure}"
    az login --identity --only-show-errors >/dev/null 2>&1 \
        || die "az login --identity failed"
    for k in key1 key2; do
        az storage account keys renew -n "$STORAGE_ACCOUNT" -g "$STORAGE_RG" \
            --key "$k" --only-show-errors >/dev/null \
            || die "renew of ${k} failed"
        log "renewed ${k}"
    done
    # No restart: backups authenticate with the VM's managed identity, so
    # nothing on this host is holding an account key any more.
    record storage-key "both account keys renewed by rotate-secrets.sh"
    log "storage-key rotated (backups use managed identity and are unaffected)" ;;

  "") die "--rotate needs a key (try --help)" ;;
  *)  die "unknown secret '${TARGET}' (try --help)" ;;
esac
