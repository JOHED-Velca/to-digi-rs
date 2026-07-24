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

if [ "$#" -eq 1 ] && [ "$1" = "info" ]; then
    exit "${FAKE_DOCKER_INFO_EXIT:-0}"
fi

if [ "$#" -eq 2 ] && [ "$1" = "compose" ] && [ "$2" = "version" ]; then
    exit "${FAKE_DOCKER_COMPOSE_VERSION_EXIT:-0}"
fi

if [ "$#" -ge 1 ] && [ "$1" = "compose" ]; then
    if printf '%s\n' "$*" | grep -Fq ' config'; then
        exit 0
    fi
    if printf '%s\n' "$*" | grep -Eq ' importer (--help|--version)$'; then
        exit "${FAKE_IMPORT_EXIT:-0}"
    fi
    printf 'LOCAL_UID=%s\n' "${LOCAL_UID:-}" >>"$log"
    printf 'LOCAL_GID=%s\n' "${LOCAL_GID:-}" >>"$log"
    printf 'TO_DIGI_RS_IMAGE=%s\n' "${TO_DIGI_RS_IMAGE:-}" >>"$log"
    printf 'compose-run-ok\n' >logs.txt
    if printf '%s\n' "$*" | grep -Fq ' importer analyze'; then
        printf 'analysis-ok\n' >analysis-report.txt
        printf '{"schema_version":1}\n' >analysis-report.json
    fi
    if printf '%s\n' "$*" | grep -Fq ' importer verify-import'; then
        printf 'verification-ok\n' >verification-report.txt
        printf '{"schema_version":1}\n' >verification-report.json
    fi
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
    mkdir -p "$dir"
    cp "$ROOT_DIR/deploy/compose.yaml" "$dir/compose.yaml"
    cp "$ROOT_DIR/deploy/import.sh" "$dir/import.sh"
    cp "$ROOT_DIR/deploy/run.sh" "$dir/run.sh"
    cp "$ROOT_DIR/deploy/config.example.toml" "$dir/config.toml"
    mkdir -p "$dir/output"
    printf 'mdb\n' >"$dir/plu.mdb"
    chmod +x "$dir/import.sh" "$dir/run.sh"
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
    "$deploy_dir/import.sh" "$@" >"$output_file" 2>&1
}

run_wrapper_with_fake_docker() {
    local deploy_dir="$1"
    local output_file="$2"
    shift 2
    local fake_dir="$TEST_ROOT/fake-bin"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-docker.log" \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 \
    PATH="$fake_dir:$PATH" \
    "$deploy_dir/run.sh" "$@" >"$output_file" 2>&1
}

test_resolves_own_directory_and_archives_output() {
    local deploy_dir="$TEST_ROOT/deploy-a"
    local output="$TEST_ROOT/output-a.txt"
    copy_deploy "$deploy_dir"
    mkdir -p "$TEST_ROOT/elsewhere"
    (cd "$TEST_ROOT/elsewhere" && run_with_fake_docker "$deploy_dir" "$output")

    assert_contains "$output" "Importer exit code: 0"
    assert_contains "$output" "$deploy_dir/output/run-"
    [ -f "$deploy_dir"/output/run-*-import/logs.txt ] || fail "import logs.txt was not archived under an import-suffixed directory"
    assert_contains "$TEST_ROOT/fake-docker.log" "TO_DIGI_RS_IMAGE=ghcr.io/johed-velca/to-digi-rs:0.6.0"
    [ -f "$deploy_dir"/output/run-*/logs.txt ] || fail "logs.txt was not archived"
    [ -f "$deploy_dir"/output/run-*/payload-previews/plu-1.json ] || fail "payload preview was not archived"
    [ ! -f "$deploy_dir/logs.txt" ] || fail "root logs.txt was not left behind"
}

test_help_and_version_do_not_require_config_or_plu() {
    local deploy_dir="$TEST_ROOT/deploy-help"
    local output="$TEST_ROOT/output-help.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml" "$deploy_dir/plu.mdb"

    run_with_fake_docker "$deploy_dir" "$output" --help

    assert_contains "$output" "Importer exit code: 0"
    assert_not_contains "$output" "Importer did not create logs.txt"
    [ -d "$deploy_dir"/output/run-*-info ] || fail "help output directory was not created under an info suffix"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer --help"

    local deploy_dir_v="$TEST_ROOT/deploy-version"
    local output_v="$TEST_ROOT/output-version.txt"
    copy_deploy "$deploy_dir_v"
    rm "$deploy_dir_v/config.toml" "$deploy_dir_v/plu.mdb"

    run_with_fake_docker "$deploy_dir_v" "$output_v" --version

    assert_contains "$output_v" "Importer exit code: 0"
    assert_not_contains "$output_v" "Importer did not create logs.txt"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer --version"
}

test_test_connection_does_not_require_plu() {
    local deploy_dir="$TEST_ROOT/deploy-test-connection"
    local output="$TEST_ROOT/output-test-connection.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/plu.mdb"

    run_with_fake_docker "$deploy_dir" "$output" test-connection

    assert_contains "$output" "Importer exit code: 0"
    [ -f "$deploy_dir"/output/run-*-test-connection/logs.txt ] || fail "test-connection output was not archived under a command-suffixed directory"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer test-connection"
}

test_cli_arguments_are_forwarded() {
    local deploy_dir="$TEST_ROOT/deploy-args"
    local output="$TEST_ROOT/output-args.txt"
    copy_deploy "$deploy_dir"

    run_with_fake_docker "$deploy_dir" "$output" import --limit 2 --continue-on-error

    [ -f "$deploy_dir"/output/run-*-import/logs.txt ] || fail "import output was not archived under a command-suffixed directory"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer import --limit 2 --continue-on-error"
}

test_analyze_archives_analysis_report() {
    local deploy_dir="$TEST_ROOT/deploy-analyze"
    local output="$TEST_ROOT/output-analyze.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml"

    run_with_fake_docker "$deploy_dir" "$output" analyze

    assert_contains "$output" "Text analysis report:"
    assert_contains "$output" "JSON analysis report:"
    [ -f "$deploy_dir"/output/run-*-analyze/analysis-report.txt ] || fail "analysis-report.txt was not archived"
    [ -f "$deploy_dir"/output/run-*-analyze/analysis-report.json ] || fail "analysis-report.json was not archived"
}

test_verify_import_archives_verification_reports() {
    local deploy_dir="$TEST_ROOT/deploy-verify-import"
    local output="$TEST_ROOT/output-verify-import.txt"
    copy_deploy "$deploy_dir"

    run_with_fake_docker "$deploy_dir" "$output" verify-import --limit 1

    assert_contains "$output" "Text verification report:"
    assert_contains "$output" "JSON verification report:"
    [ -f "$deploy_dir"/output/run-*-verify-import/verification-report.txt ] || fail "verification-report.txt was not archived"
    [ -f "$deploy_dir"/output/run-*-verify-import/verification-report.json ] || fail "verification-report.json was not archived"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer verify-import --limit 1"
}

test_missing_config_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-missing-config"
    local output="$TEST_ROOT/output-missing-config.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml"
    set +e
    run_with_fake_docker "$deploy_dir" "$output"
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing config exit code was $code"
    assert_contains "$output" "Missing required configuration file"
}

test_verify_import_missing_config_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-verify-import-missing-config"
    local output="$TEST_ROOT/output-verify-import-missing-config.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/config.toml"
    set +e
    run_with_fake_docker "$deploy_dir" "$output" verify-import
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "verify-import missing config exit code was $code"
    assert_contains "$output" "Missing required configuration file"
}

test_missing_plu_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-missing-plu"
    local output="$TEST_ROOT/output-missing-plu.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/plu.mdb"
    set +e
    run_with_fake_docker "$deploy_dir" "$output"
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing plu exit code was $code"
    assert_contains "$output" "Missing required source database"
}

test_verify_import_missing_plu_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-verify-import-missing-plu"
    local output="$TEST_ROOT/output-verify-import-missing-plu.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/plu.mdb"
    set +e
    run_with_fake_docker "$deploy_dir" "$output" verify-import
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "verify-import missing plu exit code was $code"
    assert_contains "$output" "Missing required source database"
}

test_symlinked_plu_is_rejected_when_supported() {
    local deploy_dir="$TEST_ROOT/deploy-symlink-plu"
    local output="$TEST_ROOT/output-symlink-plu.txt"
    copy_deploy "$deploy_dir"
    rm "$deploy_dir/plu.mdb"
    if ! ln -s "$TEST_ROOT/not-real.mdb" "$deploy_dir/plu.mdb" 2>/dev/null; then
        return 0
    fi
    set +e
    run_with_fake_docker "$deploy_dir" "$output"
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "symlink plu exit code was $code"
    assert_contains "$output" "not a symbolic link"
}

test_missing_docker_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-no-docker"
    local output="$TEST_ROOT/output-no-docker.txt"
    copy_deploy "$deploy_dir"
    set +e
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 DOCKER_BIN="$TEST_ROOT/does-not-exist" "$deploy_dir/import.sh" >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "missing docker exit code was $code"
    assert_contains "$output" "Required command not found"
}

test_docker_daemon_failure_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-daemon-fail"
    local output="$TEST_ROOT/output-daemon-fail.txt"
    copy_deploy "$deploy_dir"
    local fake_dir="$TEST_ROOT/fake-daemon-bin"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    set +e
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-daemon.log" FAKE_DOCKER_INFO_EXIT=1 \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 PATH="$fake_dir:$PATH" \
    "$deploy_dir/import.sh" >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "daemon failure exit code was $code"
    assert_contains "$output" "Docker daemon is not reachable"
}

test_compose_plugin_failure_fails_clearly() {
    local deploy_dir="$TEST_ROOT/deploy-compose-fail"
    local output="$TEST_ROOT/output-compose-fail.txt"
    copy_deploy "$deploy_dir"
    local fake_dir="$TEST_ROOT/fake-compose-bin"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    set +e
    FAKE_DOCKER_LOG="$TEST_ROOT/fake-compose.log" FAKE_DOCKER_COMPOSE_VERSION_EXIT=1 \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 PATH="$fake_dir:$PATH" \
    "$deploy_dir/import.sh" >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 2 ] || fail "compose failure exit code was $code"
    assert_contains "$output" "Docker Compose plugin is not available"
}

test_image_override_uid_gid_and_exit_code_are_preserved() {
    local deploy_dir="$TEST_ROOT/deploy-exit"
    local output="$TEST_ROOT/output-exit.txt"
    local log="$TEST_ROOT/fake-docker.log"
    local fake_dir="$TEST_ROOT/fake-exit-bin"
    copy_deploy "$deploy_dir"
    mkdir -p "$fake_dir"
    make_fake_docker "$fake_dir/docker"
    set +e
    FAKE_DOCKER_LOG="$log" FAKE_IMPORT_EXIT=7 TO_DIGI_RS_IMAGE=to-digi-rs:0.6.0 \
    TO_DIGI_RS_ALLOW_NON_LINUX_FOR_TESTS=1 PATH="$fake_dir:$PATH" \
    "$deploy_dir/import.sh" >"$output" 2>&1
    local code=$?
    set -e
    [ "$code" -eq 7 ] || fail "import exit code was not preserved: $code"
    assert_contains "$output" "Importer exit code: 7"
    assert_contains "$log" "TO_DIGI_RS_IMAGE=to-digi-rs:0.6.0"
    assert_contains "$log" "LOCAL_UID="
    assert_contains "$log" "LOCAL_GID="
}

test_existing_output_is_preserved() {
    local deploy_dir="$TEST_ROOT/deploy-preserve"
    local output="$TEST_ROOT/output-preserve.txt"
    copy_deploy "$deploy_dir"
    mkdir -p "$deploy_dir/output/run-old"
    printf 'old\n' >"$deploy_dir/output/run-old/logs.txt"
    run_with_fake_docker "$deploy_dir" "$output"

    [ -f "$deploy_dir/output/run-old/logs.txt" ] || fail "existing output was deleted"
}

test_run_sh_forwards_to_import_sh_with_notice() {
    local deploy_dir="$TEST_ROOT/deploy-wrapper"
    local output="$TEST_ROOT/output-wrapper.txt"
    copy_deploy "$deploy_dir"

    run_wrapper_with_fake_docker "$deploy_dir" "$output" analyze

    assert_contains "$output" "NOTICE: run.sh has been renamed to import.sh."
    assert_contains "$output" "Forwarding this command for backward compatibility."
    assert_contains "$output" "Importer exit code: 0"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer analyze"
}

test_run_sh_forwards_verify_import_with_notice() {
    local deploy_dir="$TEST_ROOT/deploy-wrapper-verify-import"
    local output="$TEST_ROOT/output-wrapper-verify-import.txt"
    copy_deploy "$deploy_dir"

    run_wrapper_with_fake_docker "$deploy_dir" "$output" verify-import --limit 1

    assert_contains "$output" "NOTICE: run.sh has been renamed to import.sh."
    assert_contains "$output" "Forwarding this command for backward compatibility."
    assert_contains "$output" "Importer exit code: 0"
    assert_contains "$TEST_ROOT/fake-docker.log" "importer verify-import --limit 1"
}

test_package_archive_contains_only_expected_files() {
    local archive
    archive="$(TO_DIGI_RS_VERSION=0.6.0 "$ROOT_DIR/scripts/package-deploy.sh")"
    [ -f "$archive" ] || fail "archive was not created"
    local listing="$TEST_ROOT/archive-list.txt"
    tar -tzf "$archive" | sort >"$listing"

    assert_contains "$listing" "to-digi-rs-deploy/compose.yaml"
    assert_contains "$listing" "to-digi-rs-deploy/config.example.toml"
    assert_contains "$listing" "to-digi-rs-deploy/import.sh"
    assert_contains "$listing" "to-digi-rs-deploy/run.sh"
    assert_contains "$listing" "to-digi-rs-deploy/README.md"
    assert_contains "$listing" "to-digi-rs-deploy/output/"
    assert_not_contains "$listing" "to-digi-rs-deploy/config.toml"
    assert_not_contains "$listing" "to-digi-rs-deploy/plu.mdb"
    assert_not_contains "$listing" "to-digi-rs-deploy/verification-report.txt"
    assert_not_contains "$listing" "to-digi-rs-deploy/verification-report.json"
    assert_not_contains "$listing" "target/"
    assert_not_contains "$listing" ".git/"
}

test_resolves_own_directory_and_archives_output
test_help_and_version_do_not_require_config_or_plu
test_test_connection_does_not_require_plu
test_cli_arguments_are_forwarded
test_analyze_archives_analysis_report
test_verify_import_archives_verification_reports
test_missing_config_fails_clearly
test_verify_import_missing_config_fails_clearly
test_missing_plu_fails_clearly
test_verify_import_missing_plu_fails_clearly
test_symlinked_plu_is_rejected_when_supported
test_missing_docker_fails_clearly
test_docker_daemon_failure_fails_clearly
test_compose_plugin_failure_fails_clearly
test_image_override_uid_gid_and_exit_code_are_preserved
test_existing_output_is_preserved
test_run_sh_forwards_to_import_sh_with_notice
test_run_sh_forwards_verify_import_with_notice
test_package_archive_contains_only_expected_files

printf 'deployment script tests passed\n'
