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

For testing, you can hard-code the Client ID and password directly in `config.toml`:

```toml
[digiweb]
client_id = "digi"
client_secret = "REPLACE_WITH_TEST_CLIENT_PASSWORD"
log_credentials_for_testing = true
```

When `log_credentials_for_testing` is `true`, `logs.txt` prints both the Client ID and Client Secret in plain text so you can confirm the connection identity.

Then run:

```bash
./to-digi-rs
```

If `client_secret` is blank in `config.toml`, the app falls back to:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
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

## DIGIweb API Settings

The local API PDF `DIGIweb_ThirdParty_API_20260607.pdf` documents the Third-Party URL shape:

```text
https://{server_ip or server name}:{port_number}/api/v1/third-party/{function}
```

For PLU create-or-update, the documented function is:

```text
plus/write
```

`config.toml` therefore defaults to:

```toml
[digiweb]
base_url = "https://192.168.0.150"
plu_upsert_path = "/api/v1/third-party/plus/write"
```

Include the port in `base_url` if the installation does not use the default HTTPS port.

The PDF says clients request a token with client ID and secret, but the token URL itself is not visible in the extracted text. Fill this from the DIGIweb SSO/OpenID Connect configuration or a known working request:

```toml
token_url = "REPLACE_WITH_CONFIRMED_OPENID_TOKEN_ENDPOINT"
```

POST requests are expected to return `201 Created` with a `Location` header. The importer polls that `Location` for `TODO`, `PROCESSING`, `SUCCESS`, or `FAIL`. `request_status_path_template` is only a fallback for non-standard responses that return an ID without a Location.

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

## DIGIweb PLU Payload

The importer serializes DIGIweb field names from the PDF, including:

```text
storeno
pluno
pludepartmentno
plugroupno
plubarcodedata
plucommname
plutexts
pluingredients
plupricemode
pluunitprice
pluusingdateprint
pluusingdateterm
pluadditionaldatas.keylabel
plunft.data
```

The PDF notes that JSON `null` values are not supported, so optional values are omitted rather than serialized as `null`.

`plubarcodetype` and `plubarcoderefno` depend on the barcode setup for the DIGIweb installation. Leave them blank to omit them, or fill them when the site requires a specific barcode type/reference:

```toml
plu_barcode_type = ""
plu_barcode_ref_no = ""
```

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

By default, secrets, full access tokens, authorization headers, passwords, and secret-bearing request bodies are not logged. During testing, setting `log_credentials_for_testing = true` intentionally logs the configured Client ID and Client Secret.
