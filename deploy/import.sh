#!/usr/bin/env bash
set -u

DEFAULT_IMAGE="ghcr.io/johed-velca/to-digi-rs:0.8.0"
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
        import)
            if has_resume_arg "$@"; then
                printf 'resume\n'
            else
                printf 'import\n'
            fi
            ;;
        analyze|verify|test-connection|sanitize) printf '%s\n' "$command" ;;
        --help|-h|--version|-V) printf 'info\n' ;;
        *) printf 'cli\n' ;;
    esac
}

has_resume_arg() {
    local arg
    for arg in "$@"; do
        case "$arg" in
            --resume|--resume=*) return 0 ;;
        esac
    done
    return 1
}

has_arg_name() {
    local name="$1"
    shift
    local arg
    for arg in "$@"; do
        case "$arg" in
            "$name") return 0 ;;
            "$name"=*) return 0 ;;
        esac
    done
    return 1
}

path_arg_value() {
    local name="$1"
    shift
    local previous=""
    local arg
    for arg in "$@"; do
        if [ "$previous" = "$name" ]; then
            printf '%s\n' "$arg"
            return 0
        fi
        case "$arg" in
            "$name"=*)
                printf '%s\n' "${arg#*=}"
                return 0
                ;;
            "$name")
                previous="$name"
                ;;
        esac
    done
    return 1
}

resume_arg_value() {
    local previous=""
    local arg
    for arg in "$@"; do
        if [ "$previous" = "--resume" ]; then
            printf '%s\n' "$arg"
            return 0
        fi
        case "$arg" in
            --resume=*)
                printf '%s\n' "${arg#--resume=}"
                return 0
                ;;
            --resume)
                previous="--resume"
                ;;
        esac
    done
    return 1
}

translate_path_arg() {
    local name="$1"
    local translated_path="$2"
    shift 2
    local previous=""
    FORWARDED_ARGS=()
    local arg
    for arg in "$@"; do
        if [ "$previous" = "$name" ]; then
            FORWARDED_ARGS+=("$translated_path")
            previous=""
            continue
        fi
        case "$arg" in
            "$name"=*)
                FORWARDED_ARGS+=("$name=$translated_path")
                ;;
            "$name")
                FORWARDED_ARGS+=("$arg")
                previous="$name"
                ;;
            *)
                FORWARDED_ARGS+=("$arg")
                ;;
        esac
    done
}

translate_resume_args() {
    local translated_manifest="$1"
    shift
    local previous=""
    FORWARDED_ARGS=()
    local arg
    for arg in "$@"; do
        if [ "$previous" = "--resume" ]; then
            FORWARDED_ARGS+=("$translated_manifest")
            previous=""
            continue
        fi
        case "$arg" in
            --resume=*)
                FORWARDED_ARGS+=("--resume=$translated_manifest")
                ;;
            --resume)
                FORWARDED_ARGS+=("$arg")
                previous="--resume"
                ;;
            *)
                FORWARDED_ARGS+=("$arg")
                ;;
        esac
    done
}

needs_config() {
    case "${1:-import}" in
        import|verify|test-connection) return 0 ;;
        *) return 1 ;;
    esac
}

needs_source_mdb() {
    case "${1:-import}" in
        import|analyze|verify|sanitize) return 0 ;;
        *) return 1 ;;
    esac
}

resolve_safe_deploy_path() {
    local value="$1"
    local label="$2"
    local host_path=""
    case "$value" in
        /*) host_path="$value" ;;
        *) host_path="$SCRIPT_DIR/$value" ;;
    esac
    local parent
    parent="$(cd -P "$(dirname "$host_path")" >/dev/null 2>&1 && pwd)" || fail "Missing $label: $value" 2
    host_path="$parent/$(basename "$host_path")"
    case "$host_path/" in
        "$SCRIPT_DIR"/*) ;;
        *) fail "Sanitization profiles must be located inside the deployment directory." 2 ;;
    esac
    [ ! -L "$host_path" ] || fail "$label must be a regular file, not a symbolic link: $host_path" 2
    [ -f "$host_path" ] || fail "Missing $label: $host_path" 2
    [ -r "$host_path" ] || fail "$label is not readable: $host_path" 2
    printf '%s\n' "$host_path"
}

require_linux() {
    if [ "${TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS:-}" = "1" ]; then
        return 0
    fi
    [ "$(uname -s)" = "Linux" ] || fail "import.sh must be executed on Linux." 2
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "Required command not found: $1" 2
}

SCRIPT_DIR="$(resolve_script_dir)" || fail "Unable to resolve the directory containing import.sh." 2
COMPOSE_FILE="$SCRIPT_DIR/compose.yaml"
OUTPUT_DIR="$SCRIPT_DIR/output"
COMMAND_LABEL="$(command_label "$@")"
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

[ -f "$COMPOSE_FILE" ] || fail "Missing compose.yaml beside import.sh: $COMPOSE_FILE" 2

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

Place the customer database beside import.sh using the exact filename plu.mdb." 2
    fi
    [ -f "$SCRIPT_DIR/plu.mdb" ] || fail "plu.mdb must be a regular file: $SCRIPT_DIR/plu.mdb" 2
    [ -r "$SCRIPT_DIR/plu.mdb" ] || fail "plu.mdb is not readable by the invoking user: $SCRIPT_DIR/plu.mdb" 2
fi

mkdir -p "$OUTPUT_DIR" || fail "Unable to create output directory: $OUTPUT_DIR" 2
[ -w "$SCRIPT_DIR" ] || fail "The invoking user cannot write to the deployment directory: $SCRIPT_DIR" 2
[ -w "$OUTPUT_DIR" ] || fail "The invoking user cannot write to the output directory: $OUTPUT_DIR" 2

rm -f "$SCRIPT_DIR/logs.txt" || fail "Unable to clean transient logs.txt before execution." 2
rm -f "$SCRIPT_DIR/analysis-report.txt" || fail "Unable to clean transient analysis-report.txt before execution." 2
rm -f "$SCRIPT_DIR/analysis-report.json" || fail "Unable to clean transient analysis-report.json before execution." 2
rm -f "$SCRIPT_DIR/sanitization-report.txt" || fail "Unable to clean transient sanitization-report.txt before execution." 2
rm -f "$SCRIPT_DIR/sanitization-report.json" || fail "Unable to clean transient sanitization-report.json before execution." 2
rm -f "$SCRIPT_DIR/sanitization-profile.snapshot.toml" || fail "Unable to clean transient sanitization-profile.snapshot.toml before execution." 2
rm -rf "$SCRIPT_DIR/payload-previews" || fail "Unable to clean transient payload-previews before execution." 2
mkdir -p "$RUN_DIR" || fail "Unable to create run output directory: $RUN_DIR" 2

export LOCAL_UID
export LOCAL_GID
export TO_DIGI_RS_IMAGE
export TO_DIGI_RS_IMPORT_MANIFEST_PATH
LOCAL_UID="$(id -u)"
LOCAL_GID="$(id -g)"
TO_DIGI_RS_IMAGE="${TO_DIGI_RS_IMAGE:-$DEFAULT_IMAGE}"
TO_DIGI_RS_IMPORT_MANIFEST_PATH=""

FORWARDED_ARGS=("$@")
resume_host_path=""
resume_container_path=""
profile_host_path=""
profile_container_path=""
if [ "$COMMAND_LABEL" = "resume" ]; then
    ! has_arg_name "--sanitize-profile" "$@" || fail "--resume cannot be combined with --sanitize-profile." 2
    resume_value="$(resume_arg_value "$@")" || fail "Missing value for --resume." 2
    case "$resume_value" in
        /*) resume_host_path="$resume_value" ;;
        *) resume_host_path="$SCRIPT_DIR/$resume_value" ;;
    esac
    resume_parent="$(cd -P "$(dirname "$resume_host_path")" >/dev/null 2>&1 && pwd)" || fail "Missing resume manifest: $resume_value" 2
    resume_host_path="$resume_parent/$(basename "$resume_host_path")"
    case "$resume_host_path" in
        "$SCRIPT_DIR"/*) ;;
        *) fail "Resume manifests must be located inside the deployment directory." 2 ;;
    esac
    [ ! -L "$resume_host_path" ] || fail "Resume manifest must be a regular file, not a symbolic link: $resume_host_path" 2
    [ -f "$resume_host_path" ] || fail "Missing resume manifest: $resume_host_path" 2
    [ -r "$resume_host_path" ] || fail "Resume manifest is not readable: $resume_host_path" 2
    [ -w "$resume_host_path" ] || fail "Resume manifest is not writable: $resume_host_path" 2
    resume_container_path="/work${resume_host_path#"$SCRIPT_DIR"}"
    translate_resume_args "$resume_container_path" "$@"
elif [ "${1:-}" = "sanitize" ]; then
    profile_value="$(path_arg_value "--profile" "$@")" || fail "sanitize requires --profile." 2
    profile_host_path="$(resolve_safe_deploy_path "$profile_value" "sanitization profile")" || exit $?
    profile_container_path="/work${profile_host_path#"$SCRIPT_DIR"}"
    translate_path_arg "--profile" "$profile_container_path" "$@"
elif has_arg_name "--sanitize-profile" "$@"; then
    profile_value="$(path_arg_value "--sanitize-profile" "$@")" || fail "Missing value for --sanitize-profile." 2
    profile_host_path="$(resolve_safe_deploy_path "$profile_value" "sanitization profile")" || exit $?
    profile_container_path="/work${profile_host_path#"$SCRIPT_DIR"}"
    translate_path_arg "--sanitize-profile" "$profile_container_path" "$@"
elif [ "${1:-import}" = "import" ] || [ "$#" -eq 0 ]; then
    TO_DIGI_RS_IMPORT_MANIFEST_PATH="/work/output/$RUN_ID/import-results.json"
fi

if [ "$COMMAND_LABEL" = "import" ] && [ -z "$TO_DIGI_RS_IMPORT_MANIFEST_PATH" ]; then
    TO_DIGI_RS_IMPORT_MANIFEST_PATH="/work/output/$RUN_ID/import-results.json"
fi

printf 'Using image: %s\n' "$TO_DIGI_RS_IMAGE"
printf 'Deployment directory: %s\n' "$SCRIPT_DIR"
printf 'Command: %s\n' "$COMMAND_LABEL"

(
    cd "$SCRIPT_DIR" || exit 2
    "$DOCKER_BIN" compose --project-name "$COMPOSE_PROJECT_NAME" -f "$COMPOSE_FILE" run --rm importer "${FORWARDED_ARGS[@]}"
)
import_exit_code=$?

log_path=""
if [ -f "$SCRIPT_DIR/logs.txt" ]; then
    mv "$SCRIPT_DIR/logs.txt" "$RUN_DIR/logs.txt" || warn "Could not archive logs.txt to $RUN_DIR"
    log_path="$RUN_DIR/logs.txt"
elif [ "$COMMAND_LABEL" != "info" ]; then
    warn "Importer did not create logs.txt."
fi

analysis_path=""
if [ -f "$SCRIPT_DIR/analysis-report.txt" ]; then
    mv "$SCRIPT_DIR/analysis-report.txt" "$RUN_DIR/analysis-report.txt" || warn "Could not archive analysis-report.txt to $RUN_DIR"
    analysis_path="$RUN_DIR/analysis-report.txt"
fi

analysis_json_path=""
if [ -f "$SCRIPT_DIR/analysis-report.json" ]; then
    mv "$SCRIPT_DIR/analysis-report.json" "$RUN_DIR/analysis-report.json" || warn "Could not archive analysis-report.json to $RUN_DIR"
    analysis_json_path="$RUN_DIR/analysis-report.json"
fi

sanitization_path=""
if [ -f "$SCRIPT_DIR/sanitization-report.txt" ]; then
    mv "$SCRIPT_DIR/sanitization-report.txt" "$RUN_DIR/sanitization-report.txt" || warn "Could not archive sanitization-report.txt to $RUN_DIR"
    sanitization_path="$RUN_DIR/sanitization-report.txt"
fi

sanitization_json_path=""
if [ -f "$SCRIPT_DIR/sanitization-report.json" ]; then
    mv "$SCRIPT_DIR/sanitization-report.json" "$RUN_DIR/sanitization-report.json" || warn "Could not archive sanitization-report.json to $RUN_DIR"
    sanitization_json_path="$RUN_DIR/sanitization-report.json"
fi

sanitization_snapshot_path=""
if [ -f "$SCRIPT_DIR/sanitization-profile.snapshot.toml" ]; then
    mv "$SCRIPT_DIR/sanitization-profile.snapshot.toml" "$RUN_DIR/sanitization-profile.snapshot.toml" || warn "Could not archive sanitization profile snapshot to $RUN_DIR"
    chmod 600 "$RUN_DIR/sanitization-profile.snapshot.toml" 2>/dev/null || true
    sanitization_snapshot_path="$RUN_DIR/sanitization-profile.snapshot.toml"
elif [ -f "$RUN_DIR/sanitization-profile.snapshot.toml" ]; then
    sanitization_snapshot_path="$RUN_DIR/sanitization-profile.snapshot.toml"
fi

if [ -d "$SCRIPT_DIR/payload-previews" ]; then
    mv "$SCRIPT_DIR/payload-previews" "$RUN_DIR/payload-previews" || warn "Could not archive payload previews to $RUN_DIR"
fi

manifest_path=""
if [ "$COMMAND_LABEL" = "import" ] && [ -f "$RUN_DIR/import-results.json" ]; then
    manifest_path="$RUN_DIR/import-results.json"
fi

snapshot_path=""
if [ "$COMMAND_LABEL" = "resume" ] && [ -n "$resume_host_path" ] && [ -f "$resume_host_path" ]; then
    cp "$resume_host_path" "$RUN_DIR/import-results.snapshot.json" || warn "Could not archive import-results snapshot to $RUN_DIR"
    chmod 600 "$RUN_DIR/import-results.snapshot.json" 2>/dev/null || true
    snapshot_path="$RUN_DIR/import-results.snapshot.json"
    manifest_path="$resume_host_path"
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
    printf 'Text analysis report:\n%s\n' "$analysis_path"
fi
if [ -n "$analysis_json_path" ]; then
    printf 'JSON analysis report:\n%s\n' "$analysis_json_path"
fi
if [ -n "$sanitization_path" ]; then
    printf 'Text sanitization report:\n%s\n' "$sanitization_path"
fi
if [ -n "$sanitization_json_path" ]; then
    printf 'JSON sanitization report:\n%s\n' "$sanitization_json_path"
fi
if [ -n "$sanitization_snapshot_path" ]; then
    printf 'Sanitization profile snapshot:\n%s\n' "$sanitization_snapshot_path"
fi
if [ -n "$manifest_path" ]; then
    printf 'Import manifest:\n%s\n' "$manifest_path"
fi
if [ -n "$snapshot_path" ]; then
    printf 'Import manifest snapshot:\n%s\n' "$snapshot_path"
fi

exit "$import_exit_code"
