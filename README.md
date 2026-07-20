# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot Rust importer for the customer PLU import portion of the Windows `ToDIGIweb` workflow.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb with OpenID Connect client credentials, submits PLUs sequentially, writes `./logs.txt`, and exits.

## Ubuntu Prerequisite

```bash
sudo apt install mdbtools
```

The application verifies these commands at startup:

```text
mdb-tables
mdb-schema
mdb-export
```

It does not install packages automatically.

## Runtime

Place these files in one directory:

```text
to-digi-rs
config.toml
plu.mdb
```

Then run:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
./to-digi-rs
```

During development:

```bash
cargo run
```

## Source File Safety

The importer checks exactly:

```text
./plu.mdb
```

It does not search recursively, accept alternate filenames, rename, delete, move, or write to the MDB. Symbolic links are rejected. The source file is opened read-only.

## DIGIweb Endpoints

The public DIGIweb Third-Party API endpoint paths were not available in the repository, and they should not be guessed. Set these in `config.toml` from confirmed DIGIweb documentation, the existing `ToDIGIweb` code, the OpenID Connect discovery document, or a known working API request:

```toml
[digiweb]
token_url = "https://192.168.0.150/CONFIRMED/TOKEN/PATH"
plu_upsert_path = "/CONFIRMED/PLU/UPSERT/PATH"
request_status_path_template = "/CONFIRMED/STATUS/PATH/{request_id}"
```

If a DIGIweb installation uses a self-signed certificate, this can be enabled explicitly:

```toml
allow_invalid_certificates = true
```

When enabled, `logs.txt` includes:

```text
WARNING: TLS certificate validation is disabled.
```

## Mapping Assumptions

Mapping rules live in `src/source/mapping.rs`. The default table names are:

```text
Pludata
PluIng
PluNut
```

`Pludata` is required. `PluIng` and `PluNut` are optional; missing optional tables produce warnings and the import continues without those details.

Column mappings are intentionally limited to documented code constants. Unknown or missing required PLU values are not replaced with fabricated defaults.

## Exit Codes

```text
0 = complete success
1 = import completed but one or more records failed
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```

## Logging

`logs.txt` is created or overwritten as early as possible. It records startup details, MDB tables and columns, validation findings, authentication result, per-record failures, counts, timestamps, and final status.

Secrets, full access tokens, authorization headers, passwords, and secret-bearing request bodies are not logged.
