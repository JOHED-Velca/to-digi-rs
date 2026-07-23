#!/usr/bin/env bash
set -u

DEFAULT_IMAGE="ghcr.io/johed-velca/to-digi-rs:0.4.0"
COMPOSE_PROJECT_NAME="to-digi-rs-import"
DOCKER_BIN="${DOCKER_BIN:-docker}"

fail() {
    printf 'ERROR: %s\n' "$1" >&2
    exit "${2:-2}"
}

warn() {
    printf 'WARNING: %s\n' "$1" >&2
}

resolve_script_dir() {
    local source="${BASH_SOURCE[0]}"
    while [ -L "$source" ]; do
        local dir
        dir="$(cd -P "$(dirname "$source")" >/dev/null 2>&1 && pwd)" || return 1
        source="$(readlink "$source")"
        case "$source" in
            /*) ;;
            *) source="$dir/$source" ;;
        esac
    done
    cd -P "$(dirname "$source")" >/dev/null 2>&1 && pwd
}

command_label() {
    local command="${1:-import}"
    case "$command" in
        import|analyze|verify|test-connection) printf '%s\n' "$command" ;;
        --help|-h|--version|-V) printf 'info\n' ;;
        *) printf 'cli\n' ;;
    esac
}

needs_config() {
    case "${1:-import}" in
        import|analyze|verify|test-connection) return 0 ;;
        *) return 1 ;;
    esac
}

needs_source_mdb() {
    case "${1:-import}" in
        import|analyze|verify) return 0 ;;
        *) return 1 ;;
    esac
}

require_linux() {
    if [ "${TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS:-}" = "1" ]; then
        return 0
    fi
    [ "$(uname -s)" = "Linux" ] || fail "run.sh must be executed on Linux." 2
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "Required command not found: $1" 2
}

SCRIPT_DIR="$(resolve_script_dir)" || fail "Unable to resolve the directory containing run.sh." 2
COMPOSE_FILE="$SCRIPT_DIR/compose.yaml"
OUTPUT_DIR="$SCRIPT_DIR/output"
COMMAND_LABEL="$(command_label "${1:-}")"
RUN_ID="run-$(date +%Y%m%d-%H%M%S)-$COMMAND_LABEL"
RUN_DIR="$OUTPUT_DIR/$RUN_ID"

if [ -e "$RUN_DIR" ]; then
    RUN_ID="${RUN_ID}-$$"
    RUN_DIR="$OUTPUT_DIR/$RUN_ID"
fi

require_linux
require_command "$DOCKER_BIN"

"$DOCKER_BIN" info >/dev/null 2>&1 || fail "Docker daemon is not reachable. Start Docker or add this user to the docker group." 2
"$DOCKER_BIN" compose version >/dev/null 2>&1 || fail "Docker Compose plugin is not available. Install the modern 'docker compose' plugin." 2

[ -f "$COMPOSE_FILE" ] || fail "Missing compose.yaml beside run.sh: $COMPOSE_FILE" 2

if needs_config "${1:-}"; then
    [ -f "$SCRIPT_DIR/config.toml" ] || fail "Missing required configuration file:
$SCRIPT_DIR/config.toml

Copy config.example.toml to config.toml and fill in the customer-specific DIGIweb values." 2
fi

if needs_source_mdb "${1:-}"; then
    [ ! -L "$SCRIPT_DIR/plu.mdb" ] || fail "plu.mdb must be a regular file, not a symbolic link: $SCRIPT_DIR/plu.mdb" 2
    if [ ! -e "$SCRIPT_DIR/plu.mdb" ]; then
        fail "Missing required source database:
$SCRIPT_DIR/plu.mdb

Place the customer database beside run.sh using the exact filename plu.mdb." 2
    fi
    [ -f "$SCRIPT_DIR/plu.mdb" ] || fail "plu.mdb must be a regular file: $SCRIPT_DIR/plu.mdb" 2
    [ -r "$SCRIPT_DIR/plu.mdb" ] || fail "plu.mdb is not readable by the invoking user: $SCRIPT_DIR/plu.mdb" 2
fi

mkdir -p "$OUTPUT_DIR" || fail "Unable to create output directory: $OUTPUT_DIR" 2
[ -w "$SCRIPT_DIR" ] || fail "The invoking user cannot write to the deployment directory: $SCRIPT_DIR" 2
[ -w "$OUTPUT_DIR" ] || fail "The invoking user cannot write to the output directory: $OUTPUT_DIR" 2

rm -f "$SCRIPT_DIR/logs.txt" || fail "Unable to clean transient logs.txt before execution." 2
rm -f "$SCRIPT_DIR/analysis-report.txt" || fail "Unable to clean transient analysis-report.txt before execution." 2
rm -rf "$SCRIPT_DIR/payload-previews" || fail "Unable to clean transient payload-previews before execution." 2
mkdir -p "$RUN_DIR" || fail "Unable to create run output directory: $RUN_DIR" 2

export LOCAL_UID
export LOCAL_GID
export TO_DIGI_RS_IMAGE
LOCAL_UID="$(id -u)"
LOCAL_GID="$(id -g)"
TO_DIGI_RS_IMAGE="${TO_DIGI_RS_IMAGE:-$DEFAULT_IMAGE}"

printf 'Using image: %s\n' "$TO_DIGI_RS_IMAGE"
printf 'Deployment directory: %s\n' "$SCRIPT_DIR"
printf 'Command: %s\n' "$COMMAND_LABEL"

(
    cd "$SCRIPT_DIR" || exit 2
    "$DOCKER_BIN" compose --project-name "$COMPOSE_PROJECT_NAME" -f "$COMPOSE_FILE" run --rm importer "$@"
)
import_exit_code=$?

log_path=""
if [ -f "$SCRIPT_DIR/logs.txt" ]; then
    mv "$SCRIPT_DIR/logs.txt" "$RUN_DIR/logs.txt" || warn "Could not archive logs.txt to $RUN_DIR"
    log_path="$RUN_DIR/logs.txt"
else
    warn "Importer did not create logs.txt."
fi

analysis_path=""
if [ -f "$SCRIPT_DIR/analysis-report.txt" ]; then
    mv "$SCRIPT_DIR/analysis-report.txt" "$RUN_DIR/analysis-report.txt" || warn "Could not archive analysis-report.txt to $RUN_DIR"
    analysis_path="$RUN_DIR/analysis-report.txt"
fi

if [ -d "$SCRIPT_DIR/payload-previews" ]; then
    mv "$SCRIPT_DIR/payload-previews" "$RUN_DIR/payload-previews" || warn "Could not archive payload previews to $RUN_DIR"
fi

latest_tmp="$OUTPUT_DIR/.latest.$$"
if ln -s "$RUN_ID" "$latest_tmp" 2>/dev/null; then
    mv -Tf "$latest_tmp" "$OUTPUT_DIR/latest" 2>/dev/null || rm -f "$latest_tmp"
fi

printf '\nCommand finished.\n'
printf 'Importer exit code: %s\n' "$import_exit_code"
if [ -n "$log_path" ]; then
    printf 'Log file:\n%s\n' "$log_path"
else
    printf 'Log file:\n<not created>\n'
fi
if [ -n "$analysis_path" ]; then
    printf 'Analysis report:\n%s\n' "$analysis_path"
fi

exit "$import_exit_code"
