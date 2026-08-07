#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d)"

cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

fail() {
    printf 'FAIL: %s\n' "$1" >&2
    exit 1
}

assert_contains() {
    local file="$1"
    local text="$2"
    grep -Fq -- "$text" "$file" || fail "expected '$text' in $file"
}

assert_not_contains() {
    local file="$1"
    local text="$2"
    ! grep -Fq -- "$text" "$file" || fail "did not expect '$text' in $file"
}

make_fake_docker() {
    local path="$1"
    cat >"$path" <<'FAKE'
#!/usr/bin/env bash
set -u

log="${FAKE_DOCKER_LOG:?}"
printf 'ARGS:%s\n' "$*" >>"$log"

if [ "$#" -eq 1 ] && [ "$1" = "--version" ]; then
    printf 'Docker version fake\n'
    exit 0
fi

if [ "$#" -eq 1 ] && [ "$1" = "info" ]; then
    exit "${FAKE_DOCKER_INFO_EXIT:-0}"
fi

if [ "$#" -ge 2 ] && [ "$1" = "image" ] && [ "$2" = "inspect" ]; then
    exit "${FAKE_DOCKER_IMAGE_INSPECT_EXIT:-0}"
fi

if [ "$#" -ge 1 ] && [ "$1" = "pull" ]; then
    printf 'pulled %s\n' "${2:-}" >>"$log"
    exit "${FAKE_DOCKER_PULL_EXIT:-0}"
fi

if [ "$#" -ge 1 ] && [ "$1" = "run" ]; then
    printf 'TO_DIGI_RS_IMAGE=%s\n' "${TO_DIGI_RS_IMAGE:-}" >>"$log"
    printf 'TO_DIGI_RS_IMPORT_MANIFEST_PATH=%s\n' "${TO_DIGI_RS_IMPORT_MANIFEST_PATH:-}" >>"$log"
    printf 'SECRET_VISIBLE=%s\n' "${TO_DIGI_RS_CLIENT_SECRET:-}" >>"$log"
    printf 'run-ok\n' >logs.txt
    if [ -n "${TO_DIGI_RS_IMPORT_MANIFEST_PATH:-}" ]; then
        manifest_path="${TO_DIGI_RS_IMPORT_MANIFEST_PATH#/work/}"
        mkdir -p "$(dirname "$manifest_path")"
        printf '{"schema_version":2,"run_status":"success"}\n' >"$manifest_path"
    fi
    joined=" $* "
    case "$joined" in
        *" analyze "*)
            printf 'analysis-ok\n' >analysis-report.txt
            printf '{"schema_version":1}\n' >analysis-report.json
            ;;
        *" discover "*)
            printf 'discovery-ok\n' >discovery-report.txt
            printf '{"schema_version":1}\n' >discovery-report.json
            ;;
        *" map-audit "*)
            printf 'mapping-ok\n' >mapping-report.txt
            printf '{"schema_version":1}\n' >mapping-report.json
            ;;
        *" profile suggest "*)
            mkdir -p profiles
            printf 'profile_version = 1\nprofile_name = "bigway"\n' >profiles/bigway.draft.toml
            printf 'recommendations-ok\n' >profile-recommendations.txt
            ;;
        *" sanitize "*)
            printf 'sanitize-ok\n' >sanitization-report.txt
            printf '{"schema_version":1}\n' >sanitization-report.json
            printf 'profile_version = 1\nprofile_name = "test"\n' >sanitization-profile.snapshot.toml
            ;;
        *" resume "*|*" --resume "*)
            resume_path=""
            previous=""
            for arg in "$@"; do
                if [ "$previous" = "--resume" ] || [ "$previous" = "resume" ]; then
                    resume_path="$arg"
                    previous=""
                    continue
                fi
                case "$arg" in
                    --resume=*) resume_path="${arg#--resume=}" ;;
                    --resume|resume) previous="$arg" ;;
                esac
            done
            if [ -n "$resume_path" ]; then
                host_resume_path="${resume_path#/work/}"
                printf '{"schema_version":2,"run_status":"success"}\n' >"$host_resume_path"
            fi
            ;;
    esac
    mkdir -p payload-previews
    printf '{"pluno":1}\n' >payload-previews/plu-1.json
    exit "${FAKE_IMPORT_EXIT:-0}"
fi

exit 0
FAKE
    chmod +x "$path"
}

copy_deploy() {
    local dir="$1"
    mkdir -p "$dir/profiles" "$dir/output"
    cp "$ROOT_DIR/deploy/to-digi" "$dir/to-digi"
    cp "$ROOT_DIR/deploy/import.sh" "$dir/import.sh"
    cp "$ROOT_DIR/deploy/run.sh" "$dir/run.sh"
    cp "$ROOT_DIR/deploy/compose.yaml" "$dir/compose.yaml"
    cp "$ROOT_DIR/deploy/config.example.toml" "$dir/config.toml"
    cp "$ROOT_DIR/profiles/example.toml" "$dir/profiles/example.toml"
    cp "$ROOT_DIR/profiles/starsky.toml" "$dir/profiles/starsky.toml"
    printf 'mdb\n' >"$dir/plu.mdb"
    chmod +x "$dir/to-digi" "$dir/import.sh" "$dir/run.sh"
}

run_with_fake_docker() {
    local deploy_dir="$1"
    local output_file="$2"
    shift 2
    local fake_dir="$TEST_ROOT/fake-bin"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-docker.log" \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 \
    PATH="$fake_dir:$PATH" \
    "$deploy_dir/to-digi" "$@" >"$output_file" 2>&1
}

test_launcher_archives_output_and_preserves_exit_code() {
    local deploy_dir="$TEST_ROOT/deploy with spaces"
    local output="$TEST_ROOT/output-import.txt"
    local fake_dir="$TEST_ROOT/fake-bin"
    local log="$TEST_ROOT/fake-docker.log"
    copy_deploy "$deploy_dir"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    set +e
    FAKE_DOCKER_LOG="$log" FAKE_IMPORT_EXIT=7 TO_DIGI_RS_IMAGE=to-digi-rs:0.9.0 \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 PATH="$fake_dir:$PATH" \
    "$deploy_dir/to-digi" import --limit 1 >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 7 ] || fail "launcher did not preserve importer exit code: $code"
    assert_contains "$output" "Using image: to-digi-rs:0.9.0"
    assert_contains "$output" "Importer exit code: 7"
    assert_contains "$log" "run --rm"
    assert_contains "$log" "--mount type=bind,src=$deploy_dir,dst=/work"
    assert_contains "$log" "import --limit 1"
    [ -f "$deploy_dir"/output/run-*-import/logs.txt ] || fail "logs.txt was not archived"
    [ -f "$deploy_dir"/output/run-*-import/import-results.json ] || fail "manifest was not archived"
}

test_help_version_and_pull_do_not_require_config_or_plu() {
    local deploy_dir="$TEST_ROOT/deploy-help"
    local output="$TEST_ROOT/output-help.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml" "$deploy_dir/plu.mdb"

    run_with_fake_docker "$deploy_dir" "$output" version
    assert_contains "$TEST_ROOT/fake-docker.log" "ghcr.io/johed-velca/to-digi-rs:0.9.0 version"

    run_with_fake_docker "$deploy_dir" "$output" pull
    assert_contains "$TEST_ROOT/fake-docker.log" "ARGS:pull ghcr.io/johed-velca/to-digi-rs:0.9.0"
}

test_missing_config_and_plu_fail_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-missing"
    local output="$TEST_ROOT/output-missing.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml"
    set +e
    run_with_fake_docker "$deploy_dir" "$output" import
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing config exit code was $code"
    assert_contains "$output" "Missing required configuration file"

    copy_deploy "$deploy_dir"
    rm "$deploy_dir/plu.mdb"
    set +e
    run_with_fake_docker "$deploy_dir" "$output" analyze
    code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing plu exit code was $code"
    assert_contains "$output" "Missing required source database"
}

test_profile_and_resume_paths_are_translated() {
    local deploy_dir="$TEST_ROOT/deploy-paths"
    local output="$TEST_ROOT/output-paths.txt"
    copy_deploy "$deploy_dir"
    run_with_fake_docker "$deploy_dir" "$output" analyze --sanitize-profile profiles/starsky.toml
    assert_contains "$TEST_ROOT/fake-docker.log" "analyze --sanitize-profile /work/profiles/starsky.toml"

    mkdir -p "$deploy_dir/output/old"
    printf '{"schema_version":2,"run_status":"incomplete"}\n' >"$deploy_dir/output/old/import-results.json"
    run_with_fake_docker "$deploy_dir" "$output" resume output/old/import-results.json --retry-failed
    assert_contains "$TEST_ROOT/fake-docker.log" "resume /work/output/old/import-results.json --retry-failed"
    [ -f "$deploy_dir"/output/run-*-resume/import-results.snapshot.json ] || fail "resume snapshot was not archived"
}

test_offline_diagnostics_archive_reports_without_config() {
    local deploy_dir="$TEST_ROOT/deploy-diagnostics"
    local output="$TEST_ROOT/output-diagnostics.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml"

    run_with_fake_docker "$deploy_dir" "$output" discover
    assert_contains "$TEST_ROOT/fake-docker.log" "discover"
    [ -f "$deploy_dir"/output/run-*-discover/discovery-report.txt ] || fail "discovery report was not archived"
    [ -f "$deploy_dir"/output/run-*-discover/discovery-report.json ] || fail "discovery JSON was not archived"

    run_with_fake_docker "$deploy_dir" "$output" map-audit --sample 2 --plu 1
    assert_contains "$TEST_ROOT/fake-docker.log" "map-audit --sample 2 --plu 1"
    [ -f "$deploy_dir"/output/run-*-map-audit/mapping-report.txt ] || fail "mapping report was not archived"
    [ -f "$deploy_dir"/output/run-*-map-audit/mapping-report.json ] || fail "mapping JSON was not archived"

    run_with_fake_docker "$deploy_dir" "$output" profile suggest --name bigway
    assert_contains "$TEST_ROOT/fake-docker.log" "profile suggest --name bigway"
    [ -f "$deploy_dir"/profiles/bigway.draft.toml ] || fail "profile draft was not left in profiles/"
    [ -f "$deploy_dir"/output/run-*-profile/profile-recommendations.txt ] || fail "profile recommendations were not archived"
}

test_doctor_checks_image_and_never_imports_data() {
    local deploy_dir="$TEST_ROOT/deploy-doctor"
    local output="$TEST_ROOT/output-doctor.txt"
    local doctor_log="$TEST_ROOT/fake-doctor.log"
    copy_deploy "$deploy_dir"
    local fake_dir="$TEST_ROOT/fake-bin"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    FAKE_DOCKER_LOG="$doctor_log" TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 \
    PATH="$fake_dir:$PATH" "$deploy_dir/to-digi" doctor >"$output" 2>&1
    assert_contains "$doctor_log" "image inspect ghcr.io/johed-velca/to-digi-rs:0.9.0"
    assert_contains "$doctor_log" "doctor --inside-container"
    assert_not_contains "$doctor_log" "import --limit"

    set +e
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-doctor-image.log" FAKE_DOCKER_IMAGE_INSPECT_EXIT=1 \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 PATH="$TEST_ROOT/fake-bin:$PATH" \
    "$deploy_dir/to-digi" doctor >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing image exit code was $code"
    assert_contains "$output" "Selected Docker image is not available locally"
}

test_wrappers_forward_to_to_digi() {
    local deploy_dir="$TEST_ROOT/deploy-wrappers"
    local output="$TEST_ROOT/output-wrapper.txt"
    copy_deploy "$deploy_dir"
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-docker.log" TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 \
    PATH="$TEST_ROOT/fake-bin:$PATH" "$deploy_dir/import.sh" analyze >"$output" 2>&1
    assert_contains "$output" "NOTICE: import.sh is a compatibility wrapper"
    assert_contains "$TEST_ROOT/fake-docker.log" "analyze"

    FAKE_DOCKER_LOG="$TEST_ROOT/fake-docker.log" TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 \
    PATH="$TEST_ROOT/fake-bin:$PATH" "$deploy_dir/run.sh" version >"$output" 2>&1
    assert_contains "$output" "NOTICE: run.sh is a compatibility wrapper"
}

test_package_archive_contains_expected_files_only() {
    local archive
    archive="$(TO_DIGI_RS_VERSION=0.9.0 "$ROOT_DIR/scripts/package-deploy.sh")"
    [ -f "$archive" ] || fail "archive was not created"
    local listing="$TEST_ROOT/archive-list.txt"
    tar -tzf "$archive" | sort >"$listing"

    assert_contains "$listing" "to-digi-rs-deploy/to-digi"
    assert_contains "$listing" "to-digi-rs-deploy/import.sh"
    assert_contains "$listing" "to-digi-rs-deploy/run.sh"
    assert_contains "$listing" "to-digi-rs-deploy/compose.yaml"
    assert_contains "$listing" "to-digi-rs-deploy/config.example.toml"
    assert_contains "$listing" "to-digi-rs-deploy/profiles/starsky.toml"
    assert_contains "$listing" "to-digi-rs-deploy/output/"
    assert_not_contains "$listing" "to-digi-rs-deploy/config.toml"
    assert_not_contains "$listing" "to-digi-rs-deploy/plu.mdb"
    assert_not_contains "$listing" "payload-previews"
    assert_not_contains "$listing" "import-results.json"
}

test_launcher_does_not_print_secrets() {
    local deploy_dir="$TEST_ROOT/deploy-secrets"
    local output="$TEST_ROOT/output-secrets.txt"
    copy_deploy "$deploy_dir"
    TO_DIGI_RS_CLIENT_SECRET="super-secret-value" run_with_fake_docker "$deploy_dir" "$output" test-connection
    assert_not_contains "$output" "super-secret-value"
}

test_launcher_archives_output_and_preserves_exit_code
test_help_version_and_pull_do_not_require_config_or_plu
test_missing_config_and_plu_fail_clearly
test_profile_and_resume_paths_are_translated
test_offline_diagnostics_archive_reports_without_config
test_doctor_checks_image_and_never_imports_data
test_wrappers_forward_to_to_digi
test_package_archive_contains_expected_files_only
test_launcher_does_not_print_secrets

printf 'deployment script tests passed\n'
