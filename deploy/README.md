# to-digi-rs Deployment Bundle

This directory is the portable customer deployment template for `to-digi-rs` v0.3.0.

## Quick Deployment

1. Download and extract `to-digi-rs-deploy-v0.3.0.tar.gz`.
2. Copy `config.example.toml` to `config.toml`.
3. Fill in the customer-specific DIGIweb values in `config.toml`.
4. Place the customer Access database beside `run.sh` using the exact filename `plu.mdb`.
5. Log in to GHCR once if the package is private.
6. Run `./run.sh`.
7. Read the printed log path under `output/run-.../logs.txt`.

Prepared runtime directory:

```text
to-digi-rs-deploy/
|-- compose.yaml
|-- run.sh
|-- config.toml
|-- plu.mdb
`-- output/
```

The release bundle ships `config.example.toml`, not a real `config.toml`, and it never includes a real MDB, credentials, tokens, logs, or payload previews.

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

## Normal Execution

Run from the deployment directory:

```bash
./run.sh
```

The script handles the Docker Compose project name, host networking, `/work` bind mount, image selection, UID/GID mapping, temporary container removal, exit-code capture, and output archiving.

The default image is:

```text
ghcr.io/johed-velca/to-digi-rs:0.3.0
```

## Local Or Offline Image Execution

For local testing or an offline customer VM, load or build a local image and override the image name without editing `compose.yaml`:

```bash
TO_DIGI_RS_IMAGE=to-digi-rs:0.3.0 ./run.sh
```

Offline transfer example:

```bash
docker save to-digi-rs:0.3.0 -o to-digi-rs-image-0.3.0.tar
docker load -i to-digi-rs-image-0.3.0.tar
TO_DIGI_RS_IMAGE=to-digi-rs:0.3.0 ./run.sh
```

## Output Locations

Each run gets a new timestamped directory:

```text
output/
|-- run-20260722-143000/
|   |-- logs.txt
|   `-- payload-previews/
`-- run-20260722-150500/
    |-- logs.txt
    `-- payload-previews/
```

Previous output is preserved. The script only removes transient root-level `logs.txt` and `payload-previews/` before starting the next run.

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

`Missing config.toml`: copy `config.example.toml` to `config.toml` and fill in the customer values.

`Missing plu.mdb`: place the source database beside `run.sh` using the exact lowercase filename `plu.mdb`.

`plu.mdb is a symbolic link`: replace it with a regular file. The importer rejects symlinked databases.

`Root-owned output`: run `./run.sh` as the intended Linux user. The script passes the invoking UID/GID into Compose so new files are not owned by root.

`DIGIweb connection failure`: verify `base_url`, `token_url`, network access from the Ubuntu host, and certificate settings.

`Self-signed certificate`: set `allow_invalid_certificates = true` only when required. The importer logs a prominent warning when certificate validation is disabled.

`Nonzero importer exit code`: open the printed `logs.txt` path and inspect the final status section.
