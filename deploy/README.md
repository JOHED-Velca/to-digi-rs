# to-digi-rs Deployment Bundle

This directory is the portable customer deployment template for `to-digi-rs` v0.7.0.

## Quick Deployment

1. Download and extract `to-digi-rs-deploy-v0.7.0.tar.gz`.
2. Place the customer Access database beside `import.sh` using the exact filename `plu.mdb`.
3. Run `./import.sh analyze` before configuring DIGIweb credentials.
4. Copy `config.example.toml` to `config.toml`.
5. Fill in the customer-specific DIGIweb values in `config.toml`.
6. Log in to GHCR once if the package is private.
7. Run `./import.sh verify`, then one of the import commands below.
8. Read the printed output path under `output/run-...-COMMAND/`.

```bash
./import.sh analyze
./import.sh import --test
./import.sh import --limit 10
./import.sh import --continue-on-error
./import.sh import --resume output/run-YYYYMMDD-HHMMSS-import/import-results.json
./import.sh test-connection
./import.sh verify
```

Prepared runtime directory:

```text
to-digi-rs-deploy/
|-- compose.yaml
|-- import.sh
|-- run.sh
|-- config.toml
|-- plu.mdb
`-- output/
```

The release bundle ships `config.example.toml`, not a real `config.toml`, and it never includes a real MDB, credentials, tokens, logs, manifests, analysis reports, or payload previews.

## Runner Rename

`run.sh` was renamed to `import.sh` in v0.5.1. New installations should use `import.sh`.

For this patch release, `run.sh` remains as a small compatibility wrapper. It prints a deprecation notice, forwards all arguments to `import.sh`, and preserves the exit code.

## First Customer-Installation Command

Use analysis as the first source-prerequisite check:

```bash
cp CUSTOMER_DATABASE.mdb plu.mdb
./import.sh analyze
```

`analyze` does not require `config.toml`, `DIGIWEB_CLIENT_SECRET`, working DIGIweb URLs, or network access. If `config.toml` is absent, the importer logs that it is using the built-in `Pludata` and `PluIng` mapping defaults.

The terminal summary prints the required departments and groups directly. Review both reports for audit detail and automation:

```text
analysis-report.txt
analysis-report.json
```

Then continue:

```bash
cp config.example.toml config.toml
# edit config.toml and export DIGIWEB_CLIENT_SECRET when ready
./import.sh verify
./import.sh import --limit 1
./import.sh import
```

## Commands

`analyze` reads and validates `plu.mdb`, writes `analysis-report.txt` and `analysis-report.json`, and does not authenticate or contact DIGIweb.

`import` is the only command that writes PLUs to DIGIweb. `--test` imports the first valid normalized PLU. `--limit N` imports the first `N` valid normalized PLUs. `--continue-on-error` keeps submitting later selected PLUs after a record failure or unknown status. Every real import creates `import-results.json` before authentication or PLU submission.

`import --resume MANIFEST` resumes a specific previous run. The manifest controls PLU selection, so do not combine resume with `--limit` or `--test`. `--retry-failed` is valid only with resume and retries only confirmed `FAILED` records.

`test-connection` authenticates only. It does not require `plu.mdb` and does not submit PLUs.

`verify` reads the source and authenticates, but does not write PLUs.

For one release, running `./import.sh` with no command still honors the old `[import]` config booleans and logs a deprecation warning. New scripts should use explicit commands.

## GHCR Login

If the image is private, authenticate the Ubuntu VM to GitHub Container Registry with a token that has only the access needed to pull the package, such as `read:packages`.

```bash
read -rsp "GitHub package token: " GHCR_TOKEN
echo
printf '%s' "$GHCR_TOKEN" |
    docker login ghcr.io \
        --username JOHED-Velca \
        --password-stdin
unset GHCR_TOKEN
```

Do not paste the token into `config.toml`, `import.sh`, shell history, or any repository file. Docker stores the login for later pulls.

## Image Selection

The default image is:

```text
ghcr.io/johed-velca/to-digi-rs:0.7.0
```

For local testing or an offline customer VM, load or build a local image and override the image name without editing `compose.yaml`:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.7.0 ./import.sh analyze
```

Offline transfer example:

```bash
docker save to-digi-rs:0.7.0 -o to-digi-rs-image-0.7.0.tar
docker load -i to-digi-rs-image-0.7.0.tar
TO_DIGI_RS_IMAGE=to-digi-rs:0.7.0 ./import.sh import --test
```

## Output Locations

Each run gets a new timestamped, command-suffixed directory:

```text
output/
|-- run-20260722-143000-analyze/
|   |-- logs.txt
|   |-- analysis-report.txt
|   `-- analysis-report.json
`-- run-20260722-150500-import/
    |-- logs.txt
    |-- import-results.json
    `-- payload-previews/
`-- run-20260722-151500-resume/
    |-- logs.txt
    `-- import-results.snapshot.json
```

Previous output is preserved. The script only removes transient root-level `logs.txt`, `analysis-report.txt`, `analysis-report.json`, and `payload-previews/` before starting the next run. It does not remove previous manifests.

## Analysis Statuses

```text
PASS = source analyzed successfully with no warnings
PASS_WITH_WARNINGS = source analyzed successfully, but nonblocking issues need review
FAIL = source, schema, extraction, or validation problems prevent safe analysis
```

Warnings exit `0`; `FAIL` exits `2`.

The JSON report is intended for automation. It includes `schema_version`, `application_version`, source summary, table summaries, department requirements, group requirements, barcode-format summaries, price-category summaries, PluIng/ingredient/nutrition summaries, structured warnings, blocking errors, recommendations, and safety confirmations.

`analyze` checks source prerequisites only. It does not confirm that departments or groups exist in DIGIweb. `verify` adds DIGIweb authentication/readiness checks without writing PLUs. `import` writes valid PLUs.

## Recovery And Resume

When an import is interrupted or incomplete, use the printed manifest path:

```bash
./import.sh import --resume output/run-20260724-143000-import/import-results.json
```

Retry confirmed failed records only:

```bash
./import.sh import --resume output/run-20260724-143000-import/import-results.json --retry-failed
```

`UNKNOWN_STATUS` and `AMBIGUOUS_SUBMISSION` records are never automatically resent. Known request ids are polled first. Only `NOT_ATTEMPTED` records are submitted during ordinary resume.

Resume validates the current `plu.mdb` size and SHA-256, the DIGIweb base URL, store number, client id, selected normalized PLUs, and canonical payload hashes before authentication. If anything changed, resume is cancelled and no API request is sent.

Manifest updates are atomic and protected by an exclusive file lock. A previous valid manifest is preserved as `import-results.json.bak`. A resume run writes a new `logs.txt` under `output/run-...-resume/` and copies the final manifest state to `import-results.snapshot.json`.

Do not edit `import-results.json` manually. Copy it for inspection or transport, but leave the original manifest unchanged for resume.

## Exit Codes

`import.sh` exits with the importer/container exit code.

```text
0 = complete success
1 = import completed but one or more submitted records failed or have unknown status
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Troubleshooting

`Docker not installed`: install Docker Engine on the Ubuntu VM.

`Docker daemon is not reachable`: start Docker or add the invoking user to the Docker group, then open a new shell.

`Docker Compose plugin is not available`: install the modern `docker compose` plugin. The old `docker-compose` command is not used.

`Image pull denied`: log in to GHCR with a package token that has `read:packages`, or use the offline `docker load` fallback.

`Missing config.toml`: copy `config.example.toml` to `config.toml` and fill in the customer values. `analyze`, `--help`, and `--version` do not require this file.

`Missing plu.mdb`: place the source database beside `import.sh` using the exact lowercase filename `plu.mdb`. `test-connection`, `--help`, and `--version` do not require the database.

`plu.mdb is a symbolic link`: replace it with a regular file. The importer rejects symlinked databases.

`Root-owned output`: run `./import.sh` as the intended Linux user. The script passes the invoking UID/GID into Compose so new files are not owned by root.

`DIGIweb connection failure`: verify `base_url`, `token_url`, network access from the Ubuntu host, and certificate settings.

`Self-signed certificate`: set `allow_invalid_certificates = true` only when required. The importer logs a prominent warning when certificate validation is disabled.

`Nonzero importer exit code`: open the printed `logs.txt` path and inspect the final status section.
