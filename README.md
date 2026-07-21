# to-digi-rs

`to-digi-rs` is a Linux-compatible, one-shot PLU importer for DIGIweb.

It reads only `./plu.mdb`, exports supported Access tables with `mdbtools`, normalizes and validates PLU records, authenticates to DIGIweb, submits PLUs sequentially, writes `./logs.txt`, and exits.

## Docker Deployment Model

The intended deployment artifact is a Docker image. The final runtime image contains:

```text
to-digi-rs release binary
mdbtools
ca-certificates
required Linux runtime libraries
```

The image does not contain `plu.mdb`, customer data, real `config.toml`, credentials, tokens, `logs.txt`, Rust build artifacts, or the Rust toolchain.

Build locally:

```bash
docker build -t to-digi-rs:0.1.5 .
```

Prepare a host work directory containing:

```text
plu.mdb
config.toml
```

Run the container with that directory mounted as `/work`:

```bash
docker run --rm \
  -v "$PWD/work:/work" \
  -e DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator' \
  to-digi-rs:0.1.5
```

The program reads `/work/plu.mdb`, reads `/work/config.toml`, writes `/work/logs.txt`, and exits with the application exit code.

## Local Container Inspection

For a packaging/startup test that does not contact DIGIweb, set:

```toml
[import]
dry_run_inspect_only = true
send_only_first_plu = true
continue_after_record_failure = false
```

Then run:

```bash
docker run --rm -v "$PWD/work:/work" to-digi-rs:0.1.5
```

This verifies that the container starts, `mdb-tables`, `mdb-schema`, and `mdb-export` are available, `/work/plu.mdb` is the exact source file, MDB tables can be read, `Pludata` can be exported, `PluIng` can be exported when present, counts are logged, and `/work/logs.txt` can be written. No authentication or API request is attempted.

## Remote Ubuntu Transfer

Build and save the image without using a public registry:

```bash
docker build -t to-digi-rs:0.1.5 .
docker save to-digi-rs:0.1.5 -o to-digi-rs-image-0.1.5.tar
```

Transfer `to-digi-rs-image-0.1.5.tar` to the remote Ubuntu device, then load it:

```bash
docker load -i to-digi-rs-image-0.1.5.tar
```

On the remote device, create a work directory containing the real `plu.mdb` and deployment `config.toml`, then run:

```bash
docker run --rm \
  --network host \
  -v "$PWD/work:/work" \
  -e DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator' \
  to-digi-rs:0.1.5
```

`--network host` is recommended for the first remote test so the container uses the Ubuntu host network path to `https://192.168.0.150`.

## First Remote API Test

Keep the first customer-network API test limited:

```toml
[import]
dry_run_inspect_only = false
send_only_first_plu = true
continue_after_record_failure = false
write_payload_preview = true
```

This sends only the first normalized PLU, stops after a record failure, logs the selected PLU number, logs the generated payload preview, and never logs credentials or tokens.

PLU submission and status-poll responses are logged before parsing. The audit log includes method, sanitized path, HTTP status, content type, `Location`, request-ID headers, whether the body is empty, and the sanitized raw response body. Authorization headers, access tokens, and client secrets are never written to the log.

For the current test source, keep:

```toml
[import]
dry_run_inspect_only = false
send_only_first_plu = true
continue_after_record_failure = false
write_payload_preview = true
```

## Configuration

Important `config.toml` fields:

```toml
[digiweb]
base_url = "https://192.168.0.150"
client_id = "digi"
client_secret = ""
token_url = "https://192.168.0.150/auth/realms/skypro/protocol/openid-connect/token"
store_number = 1
allow_invalid_certificates = true
plu_upsert_path = "/api/v1/third-party/plus/write"
request_status_path_template = ""
plu_barcode_type = ""
plu_barcode_ref_no = ""
```

Secrets should be supplied with:

```bash
export DIGIWEB_CLIENT_SECRET='secret-provided-by-the-operator'
```

`DIGIWEB_CLIENT_SECRET` takes precedence over `digiweb.client_secret`. The config value remains as a development fallback only. The application does not log client secrets, full access tokens, authorization headers, passwords, or secret-bearing request bodies.

## DIGIweb Prerequisites

Before running a PLU import, DIGIweb must already contain:

```text
referenced store
referenced department
each referenced group under the correct department
```

For the current test source:

```text
Department reference: 1
Group reference: 997
```

must exist and be active in DIGIweb, and group reference `997` must belong to department reference `1`.

The importer treats the group identity as:

```text
department reference number + group reference number
```

It does not assume group references are globally unique. It does not create groups, invent group names, or invent internal DIGIweb UUIDs. The DIGIweb API is expected to resolve:

```text
department reference + group reference -> internal DIGIweb group UUID
```

The source `Main Group Code` field is treated as the external DIGIweb group reference number. Values such as `997` are accepted by local validation as positive integers; DIGIweb remains responsible for final existence and business validation. The importer does not query PostgreSQL directly.

When `Main Group Code` is empty or whitespace only, the importer assigns group reference `997` and logs `Group default applied: yes`. Explicit values are trimmed and parsed, so `"0001"` becomes group `1`, `"25   "` becomes group `25`, and explicit `"997"` remains group `997` with `Group default applied: no`. Malformed non-empty values such as `ABC`, `-1`, `0`, `1.5`, or `99X` are rejected as row-level issues and are not defaulted.

Department references are also trimmed and parsed numerically before joins and payload generation. For example, source departments `"0001"` and `"1"` both normalize to department reference `1`, so `Pludata` and `PluIng` rows are matched by normalized `Plucode + Department` rather than raw department strings.

## Source File Safety

The importer checks exactly:

```text
./plu.mdb
```

Inside Docker, the working directory is `/work`, so this resolves to:

```text
/work/plu.mdb
```

It does not search recursively, accept alternate filenames, rename, delete, move, or write to the MDB. Symbolic links are rejected. The source file is opened read-only.

## MDB Mapping

The inspected MDB contains these relevant tables:

```text
Department
Pludata
PluIng
Maingroup
Presetkey
Pricelot
Scaledept
ScaleInfo
SText
```

`Pludata` is the primary PLU table. `PluIng` supplies both ingredient text and nutrition values. There is no required `PluNut` table.

Default mapping:

```toml
[mapping]
main_plu_table = "Pludata"
ingredient_table = "PluIng"
nutrition_table = ""
```

`Pludata` names are assembled from non-empty `Name 1` through `Name 4` values with DIGIweb `<br>` line breaks.

`PluIng` ingredients are assembled from non-empty `Ing Name 1` through `Ing Name 99` in numeric order.

`PluIng` nutrition values are parsed as written. Source values may be text and zero-padded, such as `0000`, `008`, or `690`; the importer does not apply hidden decimal scaling or unit conversion.

Unknown source fields are documented in code rather than guessed into DIGIweb payloads.

## DIGIweb PLU Payload

The importer serializes DIGIweb field names from `DIGIweb_ThirdParty_API_20260607.pdf`, including:

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

JSON `null` values are omitted because the DIGIweb PDF states that null JSON fields are not supported.

## Native Development

On Ubuntu without Docker:

```bash
sudo apt install mdbtools
cargo run
```

Running from Windows PowerShell builds a Windows executable, which cannot see `mdbtools` installed inside WSL. Use Docker or run Cargo inside Linux/WSL when validating `mdbtools`.

## Exit Codes

```text
0 = complete success
1 = import completed but one or more records failed
2 = startup, configuration, source parsing, or validation failure
3 = authentication or DIGIweb connection failure
4 = unexpected internal failure
```
