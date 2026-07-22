# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb, submits PLUs sequentially through the Third-Party API, writes `./logs.txt`, and exits.

## Current Stable Workflow

Version `0.3.0` keeps the confirmed importer behavior and adds one-command container deployment:

```text
plu.mdb
-> mdbtools inspection/export
-> Pludata + PluIng normalization
-> validation
-> DIGIweb authentication
-> POST /api/v1/third-party/plus/write
-> GET /api/thirdpartylinker/api/v1/requests/{request_id}
-> final SUCCESS/FAIL/unknown-status summary
```

Confirmed behavior remains unchanged: exact filename `plu.mdb`, read-only MDB access, `Pludata` and `PluIng` mappings, department/group normalization, group `997`, price and barcode mappings, ingredient/nutrition mapping, sequential submission, secret redaction, one-shot execution, no deletion, and no automatic department or group creation.

## Quick Deployment

The v0.3.0 deployment bundle lets the operator run the importer with:

```bash
./run.sh
```

No long Docker command is required.

1. Download and extract `to-digi-rs-deploy-v0.3.0.tar.gz`.
2. Copy `config.example.toml` to `config.toml`.
3. Fill in customer-specific DIGIweb values.
4. Place the source MDB beside `run.sh` using the exact filename `plu.mdb`.
5. Log in to GHCR if the package is private.
6. Run `./run.sh`.
7. Read the printed log path.

Prepared directory:

```text
to-digi-rs-deploy/
|-- compose.yaml
|-- run.sh
|-- config.toml
|-- plu.mdb
`-- output/
```

The template lives in [deploy](deploy). It does not include a real `config.toml`, real MDB, credentials, logs, or payload previews.

## Build The Deployment Bundle

Create the operator bundle locally:

```bash
bash scripts/package-deploy.sh
```

The archive is written to:

```text
target/release-bundles/to-digi-rs-deploy-v0.3.0.tar.gz
```

The archive contains only:

```text
to-digi-rs-deploy/
|-- compose.yaml
|-- run.sh
|-- config.example.toml
|-- README.md
`-- output/
```

## GHCR Image

The release image name is:

```text
ghcr.io/johed-velca/to-digi-rs:0.3.0
```

The deployment Compose file defaults to that image, but the image can be overridden without editing `compose.yaml`:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.3.0 ./run.sh
```

## GHCR Login

If the GHCR package is private, authenticate the Ubuntu VM with a token that has only `read:packages` or the minimum required pull access:

```bash
read -rsp "GitHub package token: " GHCR_TOKEN
echo
printf '%s' "$GHCR_TOKEN" |
    docker login ghcr.io \
        --username JOHED-Velca \
        --password-stdin
unset GHCR_TOKEN
```

Do not commit the token, pass it as a command-line argument, or put it in `config.toml`. Docker stores the login for later pulls.

## Normal Execution

From the prepared deployment directory:

```bash
./run.sh
```

`run.sh` resolves its own directory, checks Docker and Compose, validates `config.toml` and exact `plu.mdb`, exports the invoking UID/GID, invokes `docker compose run --rm importer`, preserves the importer exit code, and archives outputs.

The script uses host networking, a bind mount to `/work`, no fixed host path, no container name, no restart policy, no named volumes, no Docker socket mount, and no changes to the DIGIweb Compose project.

## Local Image Test

Build a local image without touching GHCR:

```bash
docker build --tag to-digi-rs:0.3.0 .
```

Prepare a deployment directory with `compose.yaml`, `run.sh`, `config.toml`, `plu.mdb`, and `output/`, then run:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.3.0 ./run.sh
```

## Offline Image Fallback

On a machine that has the image:

```bash
docker save to-digi-rs:0.3.0 -o to-digi-rs-image-0.3.0.tar
```

On the customer Ubuntu VM:

```bash
docker load -i to-digi-rs-image-0.3.0.tar
TO_DIGI_RS_IMAGE=to-digi-rs:0.3.0 ./run.sh
```

GHCR is not required after the image is loaded locally.

## Output Locations

Every run gets a new timestamped output directory:

```text
output/
|-- run-20260722-143000/
|   |-- logs.txt
|   `-- payload-previews/
`-- run-20260722-150500/
    |-- logs.txt
    `-- payload-previews/
```

Previous output is preserved. The script prints the final log path. If the importer fails before creating `logs.txt`, the script reports that clearly.

## Configuration Reference

Start from `deploy/config.example.toml`:

```toml
[digiweb]
base_url = "https://DIGIWEB_HOST_OR_IP"
client_id = "digi"
client_secret = ""
log_credentials_for_testing = false
token_url = "https://DIGIWEB_HOST_OR_IP/auth/realms/skypro/protocol/openid-connect/token"
store_number = 1
allow_invalid_certificates = false
plu_upsert_path = "/api/v1/third-party/plus/write"
request_status_path_template = "/api/thirdpartylinker/api/v1/requests/{request_id}"
plu_barcode_type = ""
plu_barcode_ref_no = ""

[import]
continue_after_record_failure = false
send_only_first_plu = true
dry_run_inspect_only = true
write_payload_preview = true
```

Supply secrets with:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
```

`DIGIWEB_CLIENT_SECRET` takes precedence over `digiweb.client_secret`. The config value is a development fallback only. Secrets, tokens, and authorization headers are not logged.

## Dry-Run Inspection

Use inspection mode to validate MDB extraction and normalization without authentication or API traffic:

```toml
[import]
continue_after_record_failure = false
send_only_first_plu = true
dry_run_inspect_only = true
write_payload_preview = true
```

## First-PLU Test

Use this for the first real API test:

```toml
[import]
continue_after_record_failure = false
send_only_first_plu = true
dry_run_inspect_only = false
write_payload_preview = true
```

Only the first valid normalized PLU is submitted. Remaining valid PLUs are intentionally skipped by the first-PLU limit and do not make the run `COMPLETED_WITH_ERRORS`.

## Full Import

Use this after prerequisites are confirmed:

```toml
[import]
continue_after_record_failure = true
send_only_first_plu = false
dry_run_inspect_only = false
write_payload_preview = true
```

## Payload Previews

When `write_payload_preview = true`, the importer writes pretty JSON payload previews under `/work/payload-previews/`. `run.sh` archives them with the matching timestamped run directory.

Preview files contain no credentials, tokens, or authorization headers.

## Exit Codes

```text
0 = complete success
1 = import completed but one or more submitted records failed or have unknown status
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Final Statuses

`SUCCESS` means every selected PLU finished successfully. Intentional first-PLU exclusions do not change this status.

`COMPLETED_WITH_ERRORS` means at least one selected PLU was confirmed failed, submitted with unknown final status, or left not attempted because stop-on-error was enabled.

`FAILED` means a fatal startup, configuration, source parsing, validation, authentication, or connection stage prevented the import from running normally.

`SUBMITTED_STATUS_UNKNOWN` means DIGIweb accepted a PLU request but the importer could not confirm the final asynchronous result. Do not blindly resubmit that PLU; use the logged request ID to investigate.

## Updating An Existing PLU

The confirmed DIGIweb endpoint behaves as an upsert. Re-running the same valid PLU may update the existing active PLU.

The importer does not directly delete inactive historical records. Failed early development attempts may leave inactive records in DIGIweb. Cleanup must use approved DIGIweb functionality or a controlled administrator procedure, not this importer.

## Troubleshooting

`Docker not installed`: install Docker Engine.

`Docker daemon not running`: start Docker or add the user to the Docker group, then open a new shell.

`Compose plugin missing`: install the modern `docker compose` plugin. The old `docker-compose` command is not used.

`GHCR login required` or `image pull denied`: log in with a `read:packages` token or use the offline image fallback.

`Missing config.toml`: copy `config.example.toml` to `config.toml`.

`Missing plu.mdb`: place the MDB beside `run.sh` using the exact lowercase filename.

`Root-owned output`: run `./run.sh` as the intended Linux user. The runner passes UID/GID into Compose.

`DIGIweb connection failure`: verify `base_url`, `token_url`, network routing, and certificate settings.

`Self-signed certificate`: set `allow_invalid_certificates = true` only when required. The importer logs a warning when certificate validation is disabled.

`Nonzero importer exit code`: open the printed `logs.txt` path and inspect the final section.

## Development

On Ubuntu without Docker:

```bash
sudo apt install mdbtools
cargo run
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

`.github/workflows/publish-container.yml` is configured for semantic-version tags such as `v0.3.0` and manual dispatch. It uses `GITHUB_TOKEN` to publish:

```text
ghcr.io/johed-velca/to-digi-rs:0.3.0
ghcr.io/johed-velca/to-digi-rs:v0.3.0
```

It does not publish `latest` automatically.
