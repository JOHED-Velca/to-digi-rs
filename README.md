# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, extracts supported Access tables with `mdbtools`, normalizes and validates PLU data, authenticates to DIGIweb, submits PLUs through the Third-Party API, writes audit output, and exits.

It is not a GUI, service, scheduler, folder watcher, staging database, or permanent sync loop.

## What It Imports

The current importer focuses on:

- PLUs from `Pludata`
- Ingredients from `PluIng`
- Nutrition facts from supported source columns
- Customer-specific sanitization profiles such as `starsky` and `bigway`

The source database must be named exactly:

```text
plu.mdb
```

Place it beside the launcher or run from the directory containing it.

## First-Time Setup From GHCR

Choose the image to install. For a release candidate, replace the tag with the one provided for the customer, for example `0.9.0-rc.3`.

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

This creates:

```text
to-digi
import.sh
run.sh
compose.yaml
config.example.toml
config.toml
profiles/
output/
```

## First-Run TODOs

Before the first real import:

1. Place the Access database beside `./to-digi` as `plu.mdb`.
2. Set the customer DIGIweb IP address.
3. Set the DIGIweb client secret.
4. Choose a profile when needed, for example `--profile bigway`.
5. Confirm required DIGIweb references before full import.
6. Run a dry run or one-PLU test before importing everything.

Recommended first setup command:

```bash
./to-digi import \
  --config-ip 192.168.0.150 \
  --config-secret \
  --profile bigway \
  --test
```

`--config-secret` prompts without echoing the secret, writes it to `config.toml`, creates a backup, then continues with the requested command.

For automation, avoid putting secrets in command history:

```bash
printf '%s' "$DIGI_SECRET" |
./to-digi import \
  --config-ip 192.168.0.150 \
  --config-secret-stdin \
  --profile bigway \
  --test
```

Runtime-only secret override remains supported:

```bash
export TO_DIGI_RS_CLIENT_SECRET='secret-provided-by-operator'
./to-digi import --profile bigway --test
```

## Typical Workflow

```bash
./to-digi doctor
./to-digi analyze --profile bigway
./to-digi diagnose --profile bigway --invalid-only
./to-digi dry-run --profile bigway --test
./to-digi confirm all --profile bigway
./to-digi verify --profile bigway
./to-digi import --profile bigway --test
./to-digi import --profile bigway
```

## Commands

```bash
./to-digi pull
./to-digi doctor [--pull]
./to-digi test-connection
./to-digi analyze [--raw] [--profile starsky|bigway] [--sanitize-profile profiles/custom.toml]
./to-digi discover [--timings]
./to-digi diagnose [--invalid-only] [--plu PLU_NUMBER] [--category CATEGORY]
./to-digi map-audit [--sample N] [--plu PLU_NUMBER] [--timings]
./to-digi profile suggest --name NAME
./to-digi sanitize [--profile starsky|bigway|profiles/custom.toml]
./to-digi dry-run [--limit N | --test | --plu PLU_NUMBER] [--profile starsky|bigway]
./to-digi verify [--profile starsky|bigway]
./to-digi confirm departments|groups|label-formats|all [--profile bigway] [--dry-run] [--yes]
./to-digi import [--limit N | --test | --plu PLU_NUMBER] [--profile starsky|bigway]
./to-digi import [--config-ip IP] [--config-secret | --config-secret-stdin]
./to-digi resume output/run-YYYYMMDD-HHMMSS-import/import-results.json [--retry-failed]
./to-digi version
```

`import.sh` and `run.sh` are compatibility wrappers around `to-digi`.

## Configuration

Generated `config.toml` starts small:

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

Important fields:

- `digiweb.base_url`: customer DIGIweb host, usually `https://IP_ADDRESS`
- `digiweb.client_secret`: client secret, preferably set with `--config-secret`
- `digiweb.store_number`: DIGIweb store number
- `digiweb.allow_invalid_certificates`: set `true` only for trusted self-signed installs
- `import.max_in_flight`: accepted default is `16`
- `profiles.default`: optional default profile name
- `verification.*`: operator-confirmed prerequisites

`--config-ip` updates `digiweb.base_url` and absolute customer-local `digiweb.token_url` values while preserving scheme, port, path, and query. Relative API paths remain relative.

## Profiles

Built-in profiles:

- `starsky`: fills known safe empty fields, handles Best Before defaults, and preserves validated mappings.
- `bigway`: applies customer-specific nutrition remaps and ingredient suppression.

Confirmed Bigway nutrition mapping:

```text
Ing Name 95 -> Calcium amount
Calcium     -> Calcium percent
Iron        -> Iron amount
Ing Name 96 -> Iron percent
Ing Name 97 -> Sugar amount
Ing Name 98 -> Potassium amount
Ing Name 99 -> Potassium percent
```

Without `--profile bigway`, `Ing Name 1..99` remain generic ingredient text unless another selected profile remaps them.

## Architecture

The application is intentionally split by responsibility:

```text
plu.mdb
-> source reader
-> source rows
-> normalized domain models
-> validation
-> DIGIweb payloads
-> authenticated API client
-> import manifest and logs
```

Key rules:

- The source reader never sends HTTP requests.
- The DIGIweb client never parses MDB rows.
- Validation happens before live submission.
- Failed or unknown asynchronous submissions are recorded without blind retry.
- `plu.mdb` is opened read-only and never modified.

## Modules

```text
src/config.rs          configuration loading and secret resolution
src/config_setup.rs    first-time IP and secret config updates
src/cli.rs             command-line parsing
src/source/            MDB validation, schema inspection, export, mapping
src/models/            normalized PLU, ingredient, nutrition models
src/validation/        validation issues and blocking checks
src/digiweb/           auth, payloads, API client, status polling
src/import/            import runner, manifests, final results
src/recovery/          resume state, locking, source identity checks
src/diagnostics.rs     analyze/diagnose/dry-run reporting
src/confirm.rs         operator prerequisite confirmations
src/deployment.rs      generated launcher and packaging assets
```

## Output

Each launcher run archives output under:

```text
output/run-YYYYMMDD-HHMMSS-COMMAND/
```

Common files:

```text
logs.txt
analysis-report.txt/json
diagnostics-report.txt/json
dry-run-report.txt
dry-run-manifest.json
import-results.json
payload-previews/
```

Secrets, access tokens, refresh tokens, and authorization headers must not appear in output files.

## Exit Codes

```text
0 = complete success
1 = incomplete operation or record failure
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Development

Ubuntu prerequisite when running outside Docker:

```bash
sudo apt install mdbtools
```

Useful local commands:

```bash
cargo run -- analyze --raw
cargo run -- diagnose --invalid-only
cargo run -- dry-run --test
cargo run -- test-connection
```

Required checks:

```bash
cargo fmt --check
cargo test --locked
git diff --check
bash scripts/test-deploy.sh
```

Build a deployment archive:

```bash
bash scripts/package-deploy.sh
```

Build a local Docker image:

```bash
docker build -t to-digi-rs:local .
```

## Notes

- Do not pass real secrets as command-line arguments.
- Do not rename, move, delete, or edit `plu.mdb`.
- Do not run a full import until diagnostics, dry-run, and readiness checks look correct.
- Release-candidate tags such as `0.9.0-rc.3` are pilot builds, not final releases.
