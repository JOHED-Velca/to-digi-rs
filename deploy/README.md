# to-digi-rs Deployment Bundle

This directory is the portable customer deployment template for `to-digi-rs` v0.4.0.

## Quick Deployment

1. Download and extract `to-digi-rs-deploy-v0.4.0.tar.gz`.
2. Copy `config.example.toml` to `config.toml`.
3. Fill in the customer-specific DIGIweb values in `config.toml`.
4. Place the customer Access database beside `run.sh` using the exact filename `plu.mdb` for `analyze`, `import`, or `verify`.
5. Log in to GHCR once if the package is private.
6. Run one of the commands below.
7. Read the printed output path under `output/run-...-COMMAND/`.

```bash
./run.sh analyze
./run.sh import --test
./run.sh import --limit 10
./run.sh import --continue-on-error
./run.sh test-connection
./run.sh verify
```

Prepared runtime directory:

```text
to-digi-rs-deploy/
|-- compose.yaml
|-- run.sh
|-- config.toml
|-- plu.mdb
`-- output/
```

The release bundle ships `config.example.toml`, not a real `config.toml`, and it never includes a real MDB, credentials, tokens, logs, analysis reports, or payload previews.

## Commands

`analyze` reads and validates `plu.mdb`, writes `analysis-report.txt`, and does not authenticate or contact DIGIweb.

`import` is the only command that writes PLUs to DIGIweb. `--test` imports the first valid normalized PLU. `--limit N` imports the first `N` valid normalized PLUs. `--continue-on-error` keeps submitting later selected PLUs after a record failure or unknown status.

`test-connection` authenticates only. It does not require `plu.mdb` and does not submit PLUs.

`verify` reads the source and authenticates, but does not write PLUs.

For one release, running `./run.sh` with no command still honors the old `[import]` config booleans and logs a deprecation warning. New scripts should use explicit commands.

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

Do not paste the token into `config.toml`, `run.sh`, shell history, or any repository file. Docker stores the login for later pulls.

## Image Selection

The default image is:

```text
ghcr.io/johed-velca/to-digi-rs:0.4.0
```

For local testing or an offline customer VM, load or build a local image and override the image name without editing `compose.yaml`:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.4.0 ./run.sh analyze
```

Offline transfer example:

```bash
docker save to-digi-rs:0.4.0 -o to-digi-rs-image-0.4.0.tar
docker load -i to-digi-rs-image-0.4.0.tar
TO_DIGI_RS_IMAGE=to-digi-rs:0.4.0 ./run.sh import --test
```

## Output Locations

Each run gets a new timestamped, command-suffixed directory:

```text
output/
|-- run-20260722-143000-analyze/
|   |-- logs.txt
|   `-- analysis-report.txt
`-- run-20260722-150500-import/
    |-- logs.txt
    `-- payload-previews/
```

Previous output is preserved. The script only removes transient root-level `logs.txt`, `analysis-report.txt`, and `payload-previews/` before starting the next run.

## Exit Codes

`run.sh` exits with the importer/container exit code.

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

`Missing config.toml`: copy `config.example.toml` to `config.toml` and fill in the customer values. `--help` and `--version` do not require this file.

`Missing plu.mdb`: place the source database beside `run.sh` using the exact lowercase filename `plu.mdb`. `test-connection`, `--help`, and `--version` do not require the database.

`plu.mdb is a symbolic link`: replace it with a regular file. The importer rejects symlinked databases.

`Root-owned output`: run `./run.sh` as the intended Linux user. The script passes the invoking UID/GID into Compose so new files are not owned by root.

`DIGIweb connection failure`: verify `base_url`, `token_url`, network access from the Ubuntu host, and certificate settings.

`Self-signed certificate`: set `allow_invalid_certificates = true` only when required. The importer logs a prominent warning when certificate validation is disabled.

`Nonzero importer exit code`: open the printed `logs.txt` path and inspect the final status section.
