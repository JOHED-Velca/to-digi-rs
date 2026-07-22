# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb, submits PLUs sequentially through the Third-Party API, writes `./logs.txt`, and exits.

## Current Stable Workflow

Version `0.2.1` preserves the confirmed import path:

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

The importer keeps source-file safety strict: it opens exactly `./plu.mdb` read-only, rejects symbolic links, does not scan for alternate databases, and never modifies the MDB.

The Docker runtime model uses `/work` as the working directory:

```bash
docker build -t to-digi-rs:0.2.1 .

docker run --rm \
  --network host \
  -v "$PWD/work:/work" \
  -e DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator' \
  to-digi-rs:0.2.1
```

The image contains the release binary, `mdbtools`, CA certificates, and required Linux runtime libraries. It does not contain customer MDB files, real configuration, credentials, logs, Rust build artifacts, or the Rust toolchain.

## Configuration Reference

Important `config.toml` fields:

```toml
[digiweb]
base_url = "https://192.168.0.150"
client_id = "digi"
client_secret = ""
log_credentials_for_testing = false
token_url = "https://192.168.0.150/auth/realms/skypro/protocol/openid-connect/token"
store_number = 1
allow_invalid_certificates = true
plu_upsert_path = "/api/v1/third-party/plus/write"
request_status_path_template = "/api/thirdpartylinker/api/v1/requests/{request_id}"
plu_barcode_type = ""
plu_barcode_ref_no = ""

[timeouts]
request_seconds = 30
poll_interval_seconds = 2
poll_timeout_seconds = 120

[import]
continue_after_record_failure = true
send_only_first_plu = false
dry_run_inspect_only = false
write_payload_preview = true

[mapping]
main_plu_table = "Pludata"
ingredient_table = "PluIng"
nutrition_table = ""
```

Supply secrets with:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
```

`DIGIWEB_CLIENT_SECRET` takes precedence over `digiweb.client_secret`. The config value is a development fallback only. The application does not log client secrets, full access tokens, authorization headers, passwords, or secret-bearing request bodies.

## Dry-Run Inspection

Use inspection mode to verify the container, `mdbtools`, `plu.mdb`, schema export, normalization, and validation without authentication or API traffic:

```toml
[import]
continue_after_record_failure = false
send_only_first_plu = true
dry_run_inspect_only = true
write_payload_preview = true
```

Run:

```bash
docker run --rm -v "$PWD/work:/work" to-digi-rs:0.2.1
```

The final summary should use inspection wording, for example:

```text
Source rows discovered: 5
Empty source placeholders ignored: 1
Normalized PLUs: 4
Valid PLUs identified: 4
PLUs submitted: 0
Import intentionally disabled by inspection-only mode.
FINAL STATUS: SUCCESS
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

Only the first valid normalized PLU is submitted. Remaining valid PLUs are reported as intentionally skipped by the first-PLU limit; that intentional exclusion does not make the run `COMPLETED_WITH_ERRORS`.

## Full Import

Use this for the controlled sample import after prerequisites are confirmed:

```toml
[import]
continue_after_record_failure = true
send_only_first_plu = false
dry_run_inspect_only = false
write_payload_preview = true
```

This submits all valid PLUs sequentially and continues after an individual record failure so the batch summary is complete.

For a production-safe stop-on-error run:

```toml
[import]
continue_after_record_failure = false
send_only_first_plu = false
dry_run_inspect_only = false
write_payload_preview = true
```

If a PLU fails, later selected PLUs are counted as not attempted after failure, not as confirmed failures.

## Payload Previews

When `write_payload_preview = true`, the importer writes the exact sanitized JSON payload submitted for each selected PLU:

```text
/work/payload-previews/plu-1.json
/work/payload-previews/plu-4.json
/work/payload-previews/plu-2.json
/work/payload-previews/plu-3.json
```

Preview files are pretty-printed and contain no credentials, tokens, or authorization headers. At the start of each preview-enabled run, old `payload-previews/*.json` files are removed so the directory reflects the current run.

When `write_payload_preview = false`, the importer does not create preview files.

## Understanding Exit Codes

```text
0 = complete success
1 = import completed but one or more submitted records failed or have unknown status
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Understanding Final Statuses

`SUCCESS` means every selected PLU finished successfully. Intentional first-PLU exclusions do not change this status.

`COMPLETED_WITH_ERRORS` means at least one selected PLU was confirmed failed, submitted with unknown final status, or left not attempted because stop-on-error was enabled.

`FAILED` means a fatal startup, configuration, source parsing, validation, authentication, or connection stage prevented the import from running normally.

`SUBMITTED_STATUS_UNKNOWN` is used when DIGIweb accepted a PLU request but the importer could not confirm the final asynchronous result. Do not blindly resubmit that PLU; use the logged request ID to investigate.

## Updating an Existing PLU

The confirmed DIGIweb endpoint behaves as an upsert. Re-running the same valid PLU may update the existing active PLU.

The importer does not directly delete inactive historical records. Failed early development attempts may leave inactive records in DIGIweb. Database cleanup must not be performed automatically by this importer; use approved DIGIweb functionality or a controlled administrator procedure.

## Troubleshooting

If startup fails before authentication, check that `/work/plu.mdb` exists, is a regular file, is not a symbolic link, and that the container can run `mdb-tables`, `mdb-schema`, and `mdb-export`.

If authentication fails, confirm `base_url`, `token_url`, `client_id`, and `DIGIWEB_CLIENT_SECRET`. The log intentionally shows only safe credential diagnostics.

If PLU submission succeeds but polling is unknown, inspect the request ID and status endpoint logs. Normal polling logs are concise; detailed sanitized bodies are logged only for decode failures, unexpected response shapes, failed statuses, or non-2xx responses.

If DIGIweb returns a business failure, the final summary shows a concise message such as `barcodetype_uuid is null`, while detailed sanitized diagnostics remain in `logs.txt`.

## DIGIweb Prerequisites

DIGIweb must already contain the referenced store, department, and each referenced group under the correct department. The importer does not create departments or groups and does not query PostgreSQL directly.

For the current sample source, department reference `1` and group reference `997` must exist and be active.

## MDB Mapping

The confirmed source tables are `Pludata` for PLUs and `PluIng` for both ingredients and nutrition data. `PluIng` rows are joined to PLUs by normalized `Plucode + Department`.

Confirmed mappings include department normalization, group reference normalization with default group `997` for empty main-group values, price-category mapping, barcode-format mapping, barcode-data construction, ingredient formatting, and nutrition-facts mapping.

Unknown source fields are documented in code rather than guessed into DIGIweb payloads.

## Native Development

On Ubuntu without Docker:

```bash
sudo apt install mdbtools
cargo run
```

Running from Windows PowerShell builds a Windows executable, which cannot see `mdbtools` installed inside WSL. Use Docker or run Cargo inside Linux/WSL when validating `mdbtools`.

## Remote Ubuntu Transfer

Build and save the image without using a public registry:

```bash
docker build -t to-digi-rs:0.2.1 .
docker save to-digi-rs:0.2.1 -o to-digi-rs-image-0.2.1.tar
```

Transfer `to-digi-rs-image-0.2.1.tar` to the remote Ubuntu device, then load it:

```bash
docker load -i to-digi-rs-image-0.2.1.tar
```
