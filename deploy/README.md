# to-digi-rs Deployment Bundle

This directory is the portable customer deployment template for `to-digi-rs` v0.9.0.

## Preferred Setup

New deployments no longer require cloning the repository or manually downloading scripts. Initialize an empty host directory from the Docker image:

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

Then edit `config.toml`, place `plu.mdb` beside `to-digi`, and run:

```bash
./to-digi doctor
./to-digi test-connection
./to-digi analyze
./to-digi sanitize
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
./to-digi sanitize
./to-digi verify
./to-digi import
./to-digi import --limit 1
./to-digi import --continue-on-error
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

## Configuration

Generated `config.toml` is intentionally small:

```toml
[digiweb]
base_url = "https://CHANGE_ME"
client_secret = "CHANGE_ME"
store_number = 1
allow_invalid_certificates = true

[profiles]
default = "starsky"
```

Existing full `v0.8.0` configs continue to parse. Defaults are supplied for client id, token path, PLU write path, request-status path, timeouts, mapping table names, and payload previews. `token_url` may be omitted, absolute, or a relative path resolved against `base_url`.

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

## Profiles

Initialized Starsky deployments use the Starsky profile automatically through `[profiles].default = "starsky"`.

Profile precedence:

1. `--sanitize-profile profiles/custom.toml`
2. `--profile starsky`
3. `[profiles].default`
4. No profile, where supported

Use raw analysis when needed:

```bash
./to-digi analyze --raw
```

The built-in Starsky profile matches `profiles/starsky.toml`. It preserves Best Before values `1..999`, leaves `0` disabled/default, and converts empty, malformed, negative, or greater-than-999 values to `0`.

## Output

Each command gets a separate output directory:

```text
output/run-20260722-143000-analyze/
output/run-20260722-144500-sanitize/
output/run-20260722-150500-import/
output/run-20260722-151500-resume/
```

The launcher preserves previous output and only removes transient root-level reports before the next run.

## Troubleshooting

`import.sh not found after docker pull`: run the image `init` command. Pulling an image does not create host files.

`Docker not installed`: install Docker Engine or use Docker Desktop with WSL 2.

`Docker daemon is not reachable`: start Docker or add the invoking user to the Docker group.

`Image pull denied`: authenticate with `docker login ghcr.io`.

`Missing config.toml`: run `./to-digi init` or copy `config.example.toml` to `config.toml`.

`Missing plu.mdb`: place the customer Access database beside `to-digi` using the exact lowercase filename `plu.mdb`.

`Invalid profile path`: keep external profiles inside the deployment directory. Symlinks and outside paths are rejected.

`Root-owned files`: run Docker init and `./to-digi` as the intended Linux user, not root.

`TLS failure`: verify the customer certificate or set `allow_invalid_certificates = true` only when required.

`Silent output`: v0.9.0 commands print start lines, output locations, and final status. Inspect the archived `logs.txt` for details.
