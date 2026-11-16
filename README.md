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
6. Follow the guided flow: analysis, diagnostics, reference confirmation, readiness verification, dry-run, one-PLU live test, error handling, full import, and result review.

## Install and Run on Another Device

The destination device needs a Linux shell and Docker. It does not need Git, Rust, or a copy of this repository. The commands below target Ubuntu or Debian; WSL 2 users may use Docker Desktop's WSL integration instead of installing `docker.io` inside WSL.

### 1. Install and start Docker

Run these commands as the normal user who will run imports:

```bash
sudo apt-get update
sudo apt-get install -y docker.io
sudo systemctl enable --now docker
sudo usermod -aG docker "$USER"
```

Apply the new Docker group membership by signing out and back in, or run:

```bash
newgrp docker
```

Membership in the `docker` group grants root-equivalent control of the device. Only add trusted users; otherwise keep Docker restricted and use an administrator-managed deployment.

Confirm that the Docker daemon is reachable without `sudo`:

```bash
docker info
```

Do not initialize the deployment with `sudo`; doing so creates root-owned files that `./to-digi` may not be able to update.

### 2. Authenticate to GHCR if the image is private

Public images do not require login. If `docker pull` reports `denied`, create a GitHub personal access token with `read:packages`, then log in without placing the token in shell history:

```bash
export GHCR_USER="YOUR_GITHUB_USERNAME"
read -rsp "GHCR token: " GHCR_TOKEN; echo
printf '%s' "$GHCR_TOKEN" | docker login ghcr.io -u "$GHCR_USER" --password-stdin
unset GHCR_TOKEN
```

### 3. Pull the release image

Use the release tag supplied for the customer. To test the current guided-UI prerelease:

```bash
export IMAGE="ghcr.io/johed-velca/to-digi-rs:0.10.0-alpha.6"
docker pull "$IMAGE"
```

### 4. Generate the local deployment files

Create an empty deployment directory, then run the image's `init` command. This generates `to-digi`, `config.toml`, profiles, compatibility scripts, and the output directory on the host:

```bash
mkdir -p "$HOME/digi/to-digi-rs-deploy"
cd "$HOME/digi/to-digi-rs-deploy"

docker run --rm \
  --user "$(id -u):$(id -g)" \
  --mount type=bind,src="$PWD",dst=/work \
  --workdir /work \
  "$IMAGE" \
  init
```

Confirm that the launcher exists and is executable:

```bash
test -x ./to-digi && echo "to-digi launcher is ready"
```

Pulling the image alone does not create `./to-digi`; the `init` command above is required on every new device or deployment directory.

### 5. Place the customer MDB in the required location

Copy the Access database to the exact lowercase path `~/digi/plu.mdb`. Replace `/path/to/customer/plu.mdb` with its real location:

```bash
mkdir -p "$HOME/digi"
install -m 600 /path/to/customer/plu.mdb "$HOME/digi/plu.mdb"
test -r "$HOME/digi/plu.mdb" && echo "MDB source is ready"
```

The source must be a regular file, not a symbolic link. It is mounted read-only inside the importer.

### 6. Launch the guided importer

Run the launcher from the deployment directory:

```bash
cd "$HOME/digi/to-digi-rs-deploy"
./to-digi
```

On first launch, use the guided screens to:

1. Check the deployment and `~/digi/plu.mdb`.
2. Enter the DIGIweb host and client secret.
3. Choose `Generic`, `Bigway`, or `Starsky` as appropriate.
4. Run analysis and diagnostics.
5. Confirm required departments, groups, and label formats in DIGIweb.
6. Run readiness verification and a dry run.
7. Complete a successful single-PLU live test before allowing a full import.
8. Review the completion result and files under `output/`.

For later image updates, return to the deployment directory and run:

```bash
./to-digi pull
./to-digi
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

### Destination store selection

`digiweb.store_number` remains compatible with existing configurations, but it is now a preference rather than an assumption that store 1 exists. Before any live single-PLU or bulk import, to-digi reads the active stores from DIGIweb's authenticated Registry API.

1. If exactly one active store exists, that store is selected automatically even when its number differs from `store_number`. The TUI and logs show the selected number and name.
2. If multiple active stores exist, the TUI requires an explicit choice. A valid configured `store_number` is highlighted, but it is not silently accepted. Non-interactive CLI imports use `store_number` only when it matches an active store; otherwise they stop and tell you to update the config or use the TUI.
3. If no active stores exist, or the store list cannot be read safely, the import stops before submitting a PLU.

The chosen store is applied to the dry-run payload, every PLU in the live operation, and the recovery manifest. Re-run readiness if DIGIweb store configuration changes before importing.

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
src/terminal_ui/       guided terminal UI
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
