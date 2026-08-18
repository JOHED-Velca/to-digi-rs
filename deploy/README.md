# to-digi-rs Deployment Bundle

This directory is the portable customer deployment template for `to-digi-rs` v0.9.0.

## Preferred Setup

New deployments no longer require cloning the repository or manually downloading scripts. Initialize an empty host directory from the Docker image:

```bash
IMAGE="ghcr.io/johed-velca/to-digi-rs:0.9.0"

mkdir -p ~/digi/to-digi-rs-deploy
cd ~/digi/to-digi-rs-deploy

docker pull "$IMAGE"

docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount type=bind,src="$PWD",dst=/work \
  --workdir /work \
  "$IMAGE" \
  init
```

The generated launcher, Compose file, and packaged README are pinned to the exact image that created them. Customers do not need Git, Rust, source code, or a publishing token. `TO_DIGI_RS_IMAGE` remains an advanced manual override. Release candidates are pilot builds and should not be treated as final releases.

Then place `plu.mdb` beside `to-digi` and run the first import with customer setup flags:

```bash
./to-digi import \
  --config-ip 192.168.0.150 \
  --config-secret
```

`--config-ip` writes the customer DIGIweb host into customer-local URL fields such as `digiweb.base_url` and absolute `digiweb.token_url` values. Relative endpoint paths remain relative. `--config-secret` prompts for the DIGIweb client secret without echoing it, persists it to `config.toml`, creates a timestamped backup, then continues using the updated configuration.

For non-interactive setup, read the secret from stdin rather than putting it in argv:

```bash
printf '%s' "$DIGI_SECRET" |
./to-digi import \
  --config-ip 192.168.0.150 \
  --config-secret-stdin
```

`TO_DIGI_RS_CLIENT_SECRET` remains available as a runtime environment override and is not persisted unless the operator explicitly uses `--config-secret` or `--config-secret-stdin`.

Typical follow-up commands:

```bash
./to-digi doctor
./to-digi test-connection
./to-digi analyze
./to-digi discover
./to-digi diagnose
./to-digi diagnose --plu 18
./to-digi map-audit
./to-digi profile suggest --name bigway
./to-digi sanitize
./to-digi dry-run --test
./to-digi verify
./to-digi import
```

## Files

```text
to-digi-rs-deploy/
|-- to-digi
|-- import.sh
|-- run.sh
|-- compose.yaml
|-- config.example.toml
|-- config.toml
|-- profiles/
|   |-- example.toml
|   |-- bigway.toml
|   `-- starsky.toml
`-- output/
```

`to-digi` is the primary launcher. `import.sh` and `run.sh` are compatibility wrappers that forward to `to-digi`.

The bundle never includes a real `plu.mdb`, customer credentials, logs, manifests, analysis reports, sanitization reports, or payload previews.

## Launcher Commands

```bash
./to-digi pull
./to-digi doctor [--pull]
./to-digi test-connection
./to-digi analyze [--raw]
./to-digi discover [--timings]
./to-digi diagnose [--profile bigway] [--invalid-only] [--plu PLU_NUMBER] [--category CATEGORY]
./to-digi confirm departments|groups|label-formats|all [--profile bigway] [--dry-run] [--yes]
./to-digi map-audit [--sample N] [--plu PLU_NUMBER] [--timings]
./to-digi profile suggest --name bigway
./to-digi sanitize
./to-digi dry-run [--limit N | --test | --plu PLU_NUMBER]
./to-digi verify
./to-digi import
./to-digi import --dry-run
./to-digi import --limit 1
./to-digi import --plu PLU_NUMBER
./to-digi import --continue-on-error
./to-digi import --config-ip 192.168.0.150 --config-secret
./to-digi import --config-ip 192.168.0.150 --config-secret-stdin
./to-digi resume output/run-YYYYMMDD-HHMMSS-import/import-results.json
./to-digi resume output/run-YYYYMMDD-HHMMSS-import/import-results.json --retry-failed
./to-digi version
```

The launcher:

- Resolves its own directory, including paths with spaces.
- Bind-mounts that directory to `/work`.
- Runs the container as the invoking UID/GID.
- Uses host networking for local-network DIGIweb access on Linux.
- Prints the selected image and command.
- Preserves the importer exit code.
- Archives outputs under `output/run-...-COMMAND/`.
- Never prints secrets.

Override the image without editing files:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.9.0 ./to-digi analyze
```

Pull only the selected image:

```bash
./to-digi pull
./to-digi doctor --pull
```

The launcher does not prune, stop, remove, or modify unrelated Docker resources.

`discover`, `diagnose`, `dry-run`, `map-audit`, and `profile suggest` are offline-only diagnostics. They require `plu.mdb`, but not `config.toml` or credentials, and they do not authenticate, contact DIGIweb, submit PLUs, or modify the source MDB.

`diagnose` writes exact invalid/skipped PLU details, duplicate effective barcode groups, and required label formats to `diagnostics-report.txt/json`. `diagnose --plu N` also shows local details for valid PLUs, including raw/effective Label Format, required references, ingredient/NFT counts, and payload destination summary. `dry-run` writes `dry-run-report.txt` and `dry-run-manifest.json`, may build payload previews, and always records zero API write requests. `dry-run --plu N` and `import --plu N` select that exact normalized valid PLU only; they do not substitute another PLU when the target is missing, invalid, duplicated, or excluded.

`verify` checks connectivity and import readiness, but it is fail-closed for DIGIweb prerequisites. If required departments, groups, or label formats cannot be confirmed through a supported lookup endpoint, it reports `NOT READY / UNVERIFIED REFERENCE` rather than claiming the customer is ready for import.

This version has no supported DIGIweb lookup endpoint for Department, Group, or Label Format existence. After checking those objects directly in DIGIweb, record operator confirmations with `confirm`:

```bash
./to-digi confirm all --profile bigway
./to-digi verify --profile bigway
./to-digi import --profile bigway
```

Use `--dry-run` to preview without changing `config.toml`, or `--yes` only after the operator has independently confirmed the objects. `confirm` updates only `[verification]`, preserves existing confirmations and other settings, deduplicates and sorts values, creates a backup, and never modifies `plu.mdb`.

```toml
[verification]
confirmed_departments = [2]
confirmed_groups = ["2:997", "2:998"]
confirmed_label_formats = [1, 2, 3, 4, 6, 8, 21]
```

Manual confirmation means an operator checked the object independently. It is not labeled as API-confirmed. Extra confirmations not needed by the current MDB are reported as stale warnings.

Use this safe sequence before live writes:

```bash
./to-digi diagnose --invalid-only
./to-digi dry-run --test
./to-digi confirm all --profile bigway
./to-digi verify --profile bigway
./to-digi import --profile bigway --test
```

Run the live test import only after dry-run output has been reviewed and readiness is confirmed. Source Label Format `0` resolves in memory to effective Label Format `1`; positive Label Formats remain unchanged. Effective Label Formats are treated as required server-side references.

## Configuration

Generated `config.toml` is intentionally small:

```toml
[digiweb]
base_url = "https://CHANGE_ME"
client_secret = "CHANGE_ME"
store_number = 1
allow_invalid_certificates = true

[timeouts]
poll_interval_millis = 500

[import]
max_in_flight = 16

[profiles]
default = ""

[verification]
confirmed_departments = []
confirmed_groups = []
confirmed_label_formats = []
```

Existing full `v0.8.0` configs continue to parse. Defaults are supplied for client id, token path, PLU write path, request-status path, timeouts, mapping table names, and payload previews. `token_url` may be omitted, absolute, or a relative path resolved against `base_url`.

Customer setup flags can update the generated config before import:

```bash
./to-digi import --config-ip 192.168.0.150 --config-secret
```

`--config-ip` accepts only an IP address, not a URL. It updates `digiweb.base_url` and any absolute customer-local `digiweb.token_url`, preserving scheme, port, path, and query. Relative endpoint paths stay relative. `--config_ip` is accepted as an alias.

`--config-secret` prompts without echoing and persists the secret to `config.toml`. `--config-secret-stdin` reads and persists the secret from stdin for automation. `--config_secret` and `--config_secret_stdin` are accepted aliases. The existing `TO_DIGI_RS_CLIENT_SECRET` runtime override remains unchanged and is not persisted automatically.

`[import].max_in_flight` bounds concurrent accepted DIGIweb requests. Higher values can shorten large imports but increase server load; lower values are more conservative. `max_in_flight = 1` restores the original sequential submit-then-poll behavior.

Live imports show one updating progress bar on an interactive terminal. The `./to-digi` launcher conditionally passes Docker a pseudo-TTY only when host stdout is a TTY, so direct SSH terminal runs redraw one line while redirected or piped runs stay line-oriented:

```text
Importing [██████████████████░░░░░░░░░░] 73.7% 2541/3447 | ok 2541 | fail 0 | active 6 | 11.5/s | 03:41 | ETA 01:18
```

Set `TO_DIGI_RS_ASCII_PROGRESS=1` for an ASCII-only bar. Non-interactive output emits periodic `PROGRESS ...` lines with selected, completed, success, failed, unknown, active, remaining, rate, elapsed, and ETA fields.

Configuration precedence:

1. Safe explicit CLI options
2. Environment variables
3. `config.toml`
4. Built-in defaults

Supported environment overrides:

```text
TO_DIGI_RS_BASE_URL
TO_DIGI_RS_CLIENT_SECRET
TO_DIGI_RS_CLIENT_SECRET_FILE
TO_DIGI_RS_STORE_NUMBER
TO_DIGI_RS_ALLOW_INVALID_CERTIFICATES
TO_DIGI_RS_DEFAULT_PROFILE
TO_DIGI_RS_IMAGE
```

`DIGIWEB_CLIENT_SECRET` is accepted for compatibility. Command-line secrets are discouraged because they can enter shell history or process listings.

Recommended readiness workflow:

```bash
./to-digi analyze
# Check listed Departments, Groups, and effective Label Formats in DIGIweb.
./to-digi confirm all --profile bigway
./to-digi verify --profile bigway
./to-digi import --profile bigway --test
```

`verify` writes `verify-report.txt/json`. `READY` means all eligible PLUs and references are ready. `READY_WITH_SKIPS` means eligible PLUs are ready while some source records remain intentionally excluded for customer action. `NOT_READY` blocks import before any PLU write.

## Profiles

Fresh deployments do not apply a sanitization profile automatically. Set `[profiles].default` to `"starsky"` or `"bigway"` only after choosing that customer profile.

Profile precedence:

1. `--sanitize-profile profiles/custom.toml`
2. `--profile starsky` or `--profile bigway`
3. `[profiles].default`
4. No profile, where supported

Use raw analysis when needed:

```bash
./to-digi analyze --raw
```

The built-in Starsky profile matches `profiles/starsky.toml`. It preserves Best Before values `1..999`, leaves `0` disabled/default, and converts empty, malformed, negative, or greater-than-999 values to `0`.

The built-in Bigway profile matches `profiles/bigway.toml`. It remaps customer-specific nutrition fields and suppresses those reused fields from ingredient text:

```text
Ing Name 95 -> Calcium amount
Calcium     -> Calcium percent
Iron        -> Iron amount
Ing Name 96 -> Iron percent
Ing Name 97 -> Sugar amount
Ing Name 98 -> Potassium amount
Ing Name 99 -> Potassium percent
```

This is profile-specific behavior. Without `--profile bigway`, `Ing Name 1..99` remains generic ingredient text unless a selected profile explicitly remaps a field.

## Output

Each command gets a separate output directory:

```text
output/run-20260722-143000-analyze/
output/run-20260722-143500-discover/
output/run-20260722-143700-diagnose/
output/run-20260722-143800-dry-run/
output/run-20260722-144000-map-audit/
output/run-20260722-144200-profile/
output/run-20260722-144500-sanitize/
output/run-20260722-150500-import/
output/run-20260722-151500-resume/
```

The launcher preserves previous output and only removes transient root-level reports before the next run. It archives diagnostics reports, dry-run reports, dry-run manifests, verify reports, mapping reports, sanitization reports, and payload previews when those files are produced.

## Troubleshooting

`import.sh not found after docker pull`: run the image `init` command. Pulling an image does not create host files.

`Docker not installed`: install Docker Engine or use Docker Desktop with WSL 2.

`Docker daemon is not reachable`: start Docker or add the invoking user to the Docker group.

`Image pull denied`: authenticate with `docker login ghcr.io`.

`Missing config.toml`: run `./to-digi init` or copy `config.example.toml` to `config.toml`.

`Missing plu.mdb`: place the customer Access database beside `to-digi` using the exact lowercase filename `plu.mdb`.

`dry_run_inspect_only conflict`: run `./to-digi dry-run` or remove the deprecated setting before running a live `./to-digi import`.

`verify reports NOT READY / UNVERIFIED REFERENCE`: create or confirm the listed departments, groups, and effective label formats in DIGIweb, record them in `[verification]`, then rerun readiness checks. The tool does not fabricate, auto-confirm, or call manual confirmations API-confirmed.

`Invalid profile path`: keep external profiles inside the deployment directory. Symlinks and outside paths are rejected.

`Root-owned files`: run Docker init and `./to-digi` as the intended Linux user, not root.

`TLS failure`: verify the customer certificate or set `allow_invalid_certificates = true` only when required.

`Silent output`: v0.9.0 commands print start lines, output locations, and final status. Inspect the archived `logs.txt` for details.
