# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb when needed, writes `logs.txt`, and exits. It does not run a GUI, service, scheduler, watcher, staging database, or permanent sync loop.

## Current Workflow

Version `0.9.0` preserves the validated `v0.8.0` importer behavior and adds a self-initializing deployment workflow for Ubuntu and WSL:

```text
docker pull image
-> docker run image init
-> edit generated config.toml
-> place plu.mdb
-> ./to-digi doctor
-> ./to-digi analyze
-> ./to-digi import
```

The immutable `v0.8.0` release remains `27e77faa136439a2e42f9a6ff63b17ad4ff720ac` and `ghcr.io/johed-velca/to-digi-rs:0.8.0`; do not retag it.

## First-Time Ubuntu/WSL Installation

```bash
mkdir -p ~/digi
cd ~/digi

docker login ghcr.io
docker pull ghcr.io/johed-velca/to-digi-rs:0.9.0

docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount type=bind,src="$PWD",dst=/work \
  --workdir /work \
  ghcr.io/johed-velca/to-digi-rs:0.9.0 \
  init
```

The `init` command creates:

```text
to-digi
import.sh
run.sh
compose.yaml
config.example.toml
config.toml
profiles/example.toml
profiles/starsky.toml
output/
```

Then place the customer database in the same directory using the exact filename `plu.mdb`, edit only the DIGIweb host/IP and client secret in `config.toml`, and run:

```bash
./to-digi doctor
./to-digi test-connection
./to-digi analyze
./to-digi discover
./to-digi diagnose
./to-digi map-audit
./to-digi profile suggest --name bigway
./to-digi sanitize
./to-digi diagnose --plu 18
./to-digi dry-run --test
./to-digi verify
./to-digi import
```

## Commands

```bash
./to-digi pull
./to-digi doctor [--pull]
./to-digi test-connection
./to-digi analyze [--raw] [--profile starsky] [--sanitize-profile profiles/custom.toml]
./to-digi discover [--timings]
./to-digi diagnose [--invalid-only] [--plu PLU_NUMBER] [--category CATEGORY]
./to-digi map-audit [--sample N] [--plu PLU_NUMBER] [--timings]
./to-digi profile suggest --name bigway
./to-digi sanitize [--profile starsky|profiles/custom.toml]
./to-digi dry-run [--limit N] [--test] [--profile starsky] [--sanitize-profile profiles/custom.toml]
./to-digi verify [--profile starsky] [--sanitize-profile profiles/custom.toml]
./to-digi import [--limit N] [--test] [--dry-run] [--continue-on-error]
./to-digi import [--profile starsky] [--sanitize-profile profiles/custom.toml]
./to-digi resume output/run-YYYYMMDD-HHMMSS-import/import-results.json [--retry-failed]
./to-digi version
```

`import.sh` and `run.sh` remain compatibility wrappers around `to-digi`.

`discover`, `diagnose`, `dry-run`, `map-audit`, and `profile suggest` are strictly offline diagnostics. They do not require `config.toml`, do not load credentials, do not authenticate, do not contact DIGIweb, do not submit PLUs, and do not modify `plu.mdb`.

- `discover` writes `discovery-report.txt/json` with raw MDB field-quality, department/group reference, setup, sanitization-candidate, and timing details.
- `diagnose` writes `diagnostics-report.txt/json` with exact invalid/skipped PLUs, row-level missing required values, duplicate effective barcodes, and required label formats.
- `diagnose --plu N` also reports useful local details for valid PLUs, including raw/effective Label Format, required references, derived barcode, ingredient/NFT counts, and payload destination summary.
- `dry-run` writes `dry-run-report.txt` and `dry-run-manifest.json`, builds selected payload previews when enabled, and records `api_write_requests = 0`.
- `map-audit` writes `mapping-report.txt/json` showing the current source-to-payload mappings, including ingredient versus nutrition separation and bounded payload metadata samples.
- `profile suggest --name NAME` writes `profiles/NAME.draft.toml` and `profile-recommendations.txt` without overwriting an existing draft. Only deterministic safe findings become active rules; ambiguous findings stay as comments/recommendations.

`verify` is intentionally fail-closed for external DIGIweb prerequisites. If the source requires departments, groups, or label formats and no supported read/lookup endpoint confirms them, `verify` reports `NOT READY / UNVERIFIED REFERENCE` instead of giving a false ready signal.

Because this version has no supported DIGIweb read endpoint for Department, Group, or Label Format existence, operators can explicitly record independent server-side confirmations in `[verification]`. These confirmations are manual audit evidence, not API proof:

```toml
[verification]
confirmed_departments = [2]
confirmed_groups = ["2:997", "2:998"]
confirmed_label_formats = [1, 2, 3, 4, 6, 8, 21]
```

Extra confirmations that are not required by the current MDB are reported as stale warnings only. Missing required confirmations keep `verify` and `import` fail-closed.

Safe pre-import order:

```bash
./to-digi diagnose --invalid-only
./to-digi dry-run --test
./to-digi verify
./to-digi import --test
```

Run the live test import only after dry-run output is clean enough for the customer and `verify` has not found an unverified hard reference.

## Profiles

Initialized Starsky deployments set:

```toml
[profiles]
default = "starsky"

[verification]
confirmed_departments = []
confirmed_groups = []
confirmed_label_formats = []
```

Profile precedence is deterministic:

1. Explicit external profile, such as `--sanitize-profile profiles/custom.toml`
2. Explicit built-in profile, such as `--profile starsky`
3. Deployment default profile from `[profiles].default`
4. No profile, only where that meaning is supported

Use `./to-digi analyze --raw` when you need an unsanitized source analysis. The built-in Starsky profile and `profiles/starsky.toml` are kept equivalent.

Starsky rules fill empty Department with `1`, empty Barcode with the normalized PLU code, empty Barcode Format with `05`, and empty Print Format Code with `00`. They also treat Best Before `0` as disabled/default, preserve `1..999`, and replace empty, malformed, negative, or greater-than-999 Best Before values with `0`. These rules affect DIGIweb `plusellingdateterm`; use-by fields keep their separate source mappings.

Source Label Format `0` is a confirmed default and resolves in memory to effective Label Format `1`. The raw source value remains visible in diagnostics and reports, but DIGIweb payloads and prerequisite checks use the effective `plulabelformat` value. Positive Label Formats remain unchanged and are treated as required server-side references.

## Configuration

New deployments can start with the minimal generated `config.toml`:

```toml
[digiweb]
base_url = "https://CHANGE_ME"
client_secret = "CHANGE_ME"
store_number = 1
allow_invalid_certificates = true

[profiles]
default = "starsky"
```

Existing full `v0.8.0` configuration files remain compatible. Defaults are supplied for client id, token path, PLU write path, status path, timeouts, table names, store number, and payload-preview behavior. `token_url` may be an absolute URL or a relative path resolved against `base_url`; when omitted, the standard Keycloak token path is derived from `base_url`.

Environment overrides take precedence over config values:

```text
TO_DIGI_RS_BASE_URL
TO_DIGI_RS_CLIENT_SECRET
TO_DIGI_RS_CLIENT_SECRET_FILE
TO_DIGI_RS_STORE_NUMBER
TO_DIGI_RS_ALLOW_INVALID_CERTIFICATES
TO_DIGI_RS_DEFAULT_PROFILE
TO_DIGI_RS_IMAGE
```

`DIGIWEB_CLIENT_SECRET` is still accepted for compatibility. Do not pass secrets on the command line, because they may enter shell history or process listings. Secrets, tokens, refresh tokens, and authorization headers are never printed or logged.

Manual readiness workflow:

```bash
./to-digi analyze
# Check listed Departments, Groups, and effective Label Formats in DIGIweb.
# Edit [verification] in config.toml with only independently confirmed objects.
./to-digi verify
./to-digi import --test
```

`verify` writes `verify-report.txt/json`. If all required references are manually confirmed and eligible PLUs are ready, the result is `READY`. If eligible PLUs are ready but some source records are intentionally excluded for customer action, the result is `READY_WITH_SKIPS`. Both are safe to proceed to `import --test`; `NOT_READY` blocks import.

Deprecated `[import]` command-selector values such as `send_only_first_plu` and `dry_run_inspect_only` still parse during the compatibility period, but new generated configuration does not include them. Use CLI commands and flags instead. A live `import` command refuses to run when legacy `dry_run_inspect_only = true` is still set, and points the operator to `./to-digi dry-run`.

## Output And Resume

The launcher creates a timestamped output directory for each run:

```text
output/run-20260722-143000-analyze/
output/run-20260722-143200-diagnose/
output/run-20260722-143300-dry-run/
output/run-20260722-150500-import/
output/run-20260722-151500-resume/
```

It archives logs, analysis reports, discovery reports, diagnostics reports, dry-run reports, dry-run manifests, verify reports, mapping reports, profile recommendations, sanitization reports, profile snapshots, payload previews, manifests, and resume snapshots when present. Existing output and manifests are preserved. Draft profiles remain under `profiles/` for review.

Resume with:

```bash
./to-digi resume output/run-20260722-150500-import/import-results.json
./to-digi resume output/run-20260722-150500-import/import-results.json --retry-failed
```

`UNKNOWN_STATUS` and `AMBIGUOUS_SUBMISSION` records are never automatically resent.

## Exit Codes

```text
0 = complete success
1 = incomplete operation or record failure
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Development

On Ubuntu without Docker:

```bash
sudo apt install mdbtools
cargo run -- analyze --raw
cargo run -- discover
cargo run -- diagnose --invalid-only
cargo run -- dry-run --test
cargo run -- map-audit --sample 5
cargo run -- profile suggest --name bigway
cargo run -- test-connection
```

Checks:

```bash
cargo fmt --check
cargo test --locked
bash scripts/test-deploy.sh
git diff --check
```

Package the deployment archive:

```bash
bash scripts/package-deploy.sh
```

The archive excludes `config.toml`, `plu.mdb`, logs, output, manifests, payload previews, and credentials.

## Troubleshooting

`import.sh not found after docker pull`: run the image `init` command shown above. Pulling an image does not copy host launchers by itself.

`Docker daemon is not reachable`: start Docker Desktop/Engine or add the Linux user to the `docker` group, then open a new shell.

`Image pull denied`: run `docker login ghcr.io` with a token that can read the package, then `./to-digi pull`.

`Missing config.toml`: run `./to-digi init` again or copy `config.example.toml` to `config.toml`, then fill in customer values.

`Missing plu.mdb`: place the source database beside `./to-digi` using the exact lowercase filename `plu.mdb`.

`Self-signed certificate`: set `allow_invalid_certificates = true` only when required. The importer logs a warning when certificate validation is disabled.
