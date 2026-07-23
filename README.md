# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb when the chosen command needs it, writes `./logs.txt`, and exits.

## Current Workflow

Version `0.5.1` keeps the confirmed MDB mappings and DIGIweb API contract, then expands `analyze` into an offline MDB prerequisite report:

```text
plu.mdb
-> mdbtools inspection/export
-> Pludata + PluIng normalization
-> validation
-> DIGIweb authentication when needed
-> POST /api/v1/third-party/plus/write only for import
-> GET /api/thirdpartylinker/api/v1/requests/{request_id}
-> final SUCCESS/FAIL/unknown-status summary
```

Confirmed behavior remains unchanged: exact filename `plu.mdb`, read-only MDB access, `Pludata` and `PluIng` mappings, department/group normalization, group `997`, price and barcode mappings, ingredient/nutrition mapping, sequential submission, secret redaction, one-shot execution, no deletion, and no automatic department or group creation.

## Commands

```bash
to-digi-rs analyze
to-digi-rs import [--limit N] [--test] [--continue-on-error]
to-digi-rs test-connection
to-digi-rs verify
```

`analyze` reads and validates `plu.mdb`, writes `analysis-report.txt` and `analysis-report.json`, and does not authenticate or contact DIGIweb. It can run before DIGIweb credentials or URLs are finalized, and it can run without `config.toml` by using built-in source mapping defaults.

`import` is the only command that writes PLUs to DIGIweb. `--limit N` imports only the first `N` valid normalized PLUs. `--test` is a convenience alias for `--limit 1`. By default the importer stops after the first selected record failure or unknown final status; `--continue-on-error` keeps submitting later selected PLUs.

`test-connection` authenticates to DIGIweb and does not require `plu.mdb`.

`verify` reads and validates the source, then authenticates to DIGIweb. It does not write PLUs and reports import readiness.

For one release, running with no command still honors the old `[import]` config booleans and logs a deprecation warning. New automation should use explicit commands.

## First Customer-Installation Command

Run source analysis before configuring credentials or attempting an import:

```bash
cp CUSTOMER_DATABASE.mdb plu.mdb
./import.sh analyze
```

`config.toml` is optional for `analyze`; when it is absent, the tool uses the built-in mappings for `Pludata` and `PluIng`. The terminal summary prints the main department and group requirements directly. The detailed reports also identify barcode formats, price categories, PluIng matching statistics, ingredient/nutrition availability, source reference-table warnings, and recommended installation actions.

Recommended installation sequence:

```text
1. Extract the deployment bundle.
2. Place the customer database beside import.sh as plu.mdb.
3. Run ./import.sh analyze.
4. Review analysis-report.txt or analysis-report.json.
5. Prepare departments and groups in DIGIweb.
6. Configure authentication.
7. Run ./import.sh verify.
8. Run ./import.sh import --limit 1.
9. Run ./import.sh import.
```

## Quick Deployment

The v0.5.1 deployment bundle lets the operator run the importer with one command:

```bash
./import.sh analyze
./import.sh import --test
./import.sh import
./import.sh test-connection
./import.sh verify
```

1. Download and extract `to-digi-rs-deploy-v0.5.1.tar.gz`.
2. Place the source MDB beside `import.sh` using the exact filename `plu.mdb`.
3. Run `./import.sh analyze`.
4. Copy `config.example.toml` to `config.toml`.
5. Fill in customer-specific DIGIweb values.
6. Log in to GHCR if the package is private.
7. Run `./import.sh verify`, then the desired import command.
8. Read the printed output path under `output/run-...-COMMAND/`.

The template lives in [deploy](deploy). It does not include a real `config.toml`, real MDB, credentials, logs, analysis reports, or payload previews.

## Runner Rename

`run.sh` was renamed to `import.sh` in v0.5.1. New installations should use `import.sh`.

For this patch release, `run.sh` remains as a small compatibility wrapper. It prints a deprecation notice, forwards all arguments to `import.sh`, and preserves the exit code.

## Analysis Reports

`analyze` creates:

```text
analysis-report.txt
analysis-report.json
logs.txt
```

Text report sections are stable and concise:

```text
1. Source summary
2. Source tables
3. PLU validation
4. Required departments
5. Required groups
6. Barcode analysis
7. Price-category analysis
8. Ingredient and nutrition analysis
9. Source-reference-table checks
10. Warnings
11. Blocking errors
12. Recommended installation actions
13. Safety confirmation
```

The JSON report has `schema_version: 1`, `application_version`, source and summary blocks, arrays for departments/groups/barcode formats/price categories, structured warnings, blocking errors, recommendations, and a safety block. Arrays are sorted deterministically where order matters so automation can compare reports between runs.

Analysis statuses:

```text
PASS = source analyzed successfully with no warnings
PASS_WITH_WARNINGS = source analyzed successfully, but nonblocking issues need review
FAIL = source, schema, extraction, or validation problems prevent safe analysis
```

Warnings still exit `0`; `FAIL` exits `2`.

`analyze` checks source prerequisites only. It does not claim that departments or groups already exist in DIGIweb. `verify` adds DIGIweb authentication and import-readiness checks but still does not write PLUs. `import` writes valid PLUs.

## Build The Deployment Bundle

```bash
bash scripts/package-deploy.sh
```

The archive is written to:

```text
target/release-bundles/to-digi-rs-deploy-v0.5.1.tar.gz
```

## Image Names

```text
to-digi-rs:0.5.1
ghcr.io/johed-velca/to-digi-rs:0.5.1
```

The deployment Compose file defaults to the GHCR image, but the image can be overridden:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.5.1 ./import.sh analyze
```

## Configuration

Start from `deploy/config.example.toml`. Supply secrets with:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
```

`DIGIWEB_CLIENT_SECRET` takes precedence over `digiweb.client_secret`. The config value is a development fallback only. Secrets, tokens, and authorization headers are not logged.

The old `[import]` options are deprecated as command selectors. Explicit CLI flags override them:

```toml
[import]
continue_after_record_failure = false
send_only_first_plu = true
dry_run_inspect_only = true
write_payload_preview = true
```

Use `import --test` for the first real API submission, then `import` for the full sequential import once prerequisites are confirmed.

## Output Locations

Every deployment run gets a command-suffixed output directory:

```text
output/
|-- run-20260722-143000-analyze/
|   |-- logs.txt
|   |-- analysis-report.txt
|   `-- analysis-report.json
`-- run-20260722-150500-import/
    |-- logs.txt
    `-- payload-previews/
```

Previous output is preserved. The script prints the final log path plus both analysis report paths when present.

## Exit Codes

```text
0 = complete success
1 = import completed but one or more submitted records failed or have unknown status
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Development

On Ubuntu without Docker:

```bash
sudo apt install mdbtools
cargo run -- analyze
cargo run -- import --test
```

Running Cargo from Windows PowerShell builds a Windows executable, which cannot see `mdbtools` installed inside WSL. Use Docker or run Cargo inside Linux/WSL when validating `mdbtools`.

Run checks:

```bash
cargo fmt --check
cargo test
bash scripts/test-deploy.sh
```

Validate Compose from the deployment template:

```bash
cd deploy
LOCAL_UID="$(id -u)" LOCAL_GID="$(id -g)" docker compose config
```

## Container Publishing

`.github/workflows/publish-container.yml` is configured for semantic-version tags such as `v0.5.1` and manual dispatch. It publishes versioned images only and does not publish `latest` automatically.
