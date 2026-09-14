# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb. It reads the customer Access database, normalizes and validates PLUs, submits them through the DIGIweb Third-Party API, writes audit artifacts, and exits.

It is not a GUI, service, scheduler, folder watcher, staging database, or permanent sync loop.

## Quick Start

Place the customer MDB on the host at:

```text
~/digi/plu.mdb
```

Then run the guided terminal workflow:

```bash
./to-digi
```

First-download TODOs:

1. Put the customer database at `~/digi/plu.mdb`.
2. Set the DIGIweb IP address.
3. Set the DIGIweb client secret.
4. Choose the customer profile when needed, such as `bigway` or `starsky`.
5. Confirm required departments, groups, and label formats in DIGIweb.
6. Run analysis, diagnostics, dry-run, and a one-PLU live test before a full import.

## Download the GHCR Image

Use the release tag provided for the customer:

```bash
IMAGE="ghcr.io/johed-velca/to-digi-rs:0.9.0"
docker pull "$IMAGE"
```

Initialize a deployment directory:

```bash
mkdir -p ~/digi/to-digi-rs-deploy
cd ~/digi/to-digi-rs-deploy

docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount type=bind,src="$PWD",dst=/work \
  --workdir /work \
  "$IMAGE" \
  init
```

The generated `to-digi` launcher mounts:

```text
host:      $HOME/digi/plu.mdb
container: /work/plu.mdb read-only
```

## Commands

Recommended:

```bash
./to-digi
```

Advanced CLI commands remain available:

```bash
./to-digi pull
./to-digi doctor [--pull]
./to-digi test-connection
./to-digi analyze [--raw] [--profile starsky|bigway]
./to-digi discover [--timings]
./to-digi diagnose [--invalid-only] [--plu PLU_NUMBER]
./to-digi map-audit [--sample N] [--plu PLU_NUMBER]
./to-digi profile suggest --name NAME
./to-digi sanitize [--profile starsky|bigway|profiles/custom.toml]
./to-digi dry-run [--limit N | --test | --plu PLU_NUMBER] [--profile starsky|bigway]
./to-digi verify [--profile starsky|bigway]
./to-digi confirm departments|groups|label-formats|all [--profile bigway]
./to-digi import [--limit N | --test | --plu PLU_NUMBER] [--profile starsky|bigway]
./to-digi import [--config-ip IP] [--config-secret | --config-secret-stdin]
./to-digi resume output/run-YYYYMMDD-HHMMSS-import/import-results.json [--retry-failed]
./to-digi version
```

## Configuration

Generated `config.toml` contains the customer settings. The fields most often changed first are:

```toml
[digiweb]
base_url = "https://CHANGE_ME"
client_secret = "CHANGE_ME"
store_number = 1
allow_invalid_certificates = true

[profiles]
default = ""
```

Use the guided UI or these safe setup commands:

```bash
./to-digi import --config-ip 192.168.0.150 --config-secret --test
```

For non-interactive setup:

```bash
printf '%s' "$DIGI_SECRET" |
./to-digi import --config-ip 192.168.0.150 --config-secret-stdin --test
```

Do not put real secrets directly in shell history.

## Architecture

```text
launcher
-> Docker container
-> fixed /work/plu.mdb reader
-> source rows
-> normalized domain models
-> validation
-> DIGIweb payloads
-> authenticated API client
-> import manifest, reports, and logs
```

The source reader never sends HTTP requests. The DIGIweb client never parses MDB rows. Validation and readiness checks happen before live submission. Failed or unknown asynchronous submissions are recorded without blind retry.

## Modules

```text
src/cli.rs             command-line parsing
src/terminal_ui.rs     guided terminal UI
src/config.rs          configuration and secret resolution
src/config_setup.rs    IP and secret config updates
src/source/            MDB validation, schema inspection, export, mapping
src/models/            normalized PLU, ingredient, nutrition models
src/validation/        validation issues and blocking checks
src/digiweb/           auth, payloads, API client, status polling
src/import/            import runner, manifests, final results
src/recovery/          resume state, locking, source identity checks
src/diagnostics.rs     analyze, diagnose, dry-run reporting
src/confirm.rs         operator prerequisite confirmations
src/deployment.rs      generated launcher and packaging assets
```

## Output

Launcher runs archive artifacts under:

```text
output/run-YYYYMMDD-HHMMSS-COMMAND/
```

Common artifacts include `logs.txt`, analysis/diagnostic reports, dry-run manifests, import manifests, and payload previews. Secrets, access tokens, and authorization headers must not appear in output files.

## Development

Ubuntu prerequisite outside Docker:

```bash
sudo apt install mdbtools
```

Useful checks:

```bash
cargo fmt --check
cargo test --locked
git diff --check
bash scripts/test-deploy.sh
```
