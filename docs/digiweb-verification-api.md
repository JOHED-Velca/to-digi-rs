# DIGIweb Post-Import Verification API Discovery

This document records the endpoint discovery for `to-digi-rs` v0.6.0 post-import verification.

The important conclusion is that a supported read-only PLU lookup API has not been confirmed from the available materials. Because of that, `verify-import` stops at the API discovery gate and writes `verification-report.txt` and `verification-report.json` instead of guessing an endpoint.

## Evidence Reviewed

The following project materials were inspected:

- `C:\DIGI\DIGIweb_ThirdParty_API_20260607.pdf`
- Extracted PDF text generated locally under `target\DIGIweb_ThirdParty_API_20260607.txt`
- Existing Rust DIGIweb client code
- Existing Rust configuration defaults
- Working VB.NET source under `C:\DIGI\GIT\ToDIGIweb`
- Previously confirmed DIGIweb request and response examples captured in the project history

## Confirmed Write And Status APIs

### PLU Write

Endpoint:

```text
POST /api/v1/third-party/plus/write
```

Evidence:

- The Third-Party API PDF documents the PLU write operation under the PLU write section.
- The VB.NET importer submits PLU write requests to this path.
- The Rust importer has successfully received `201 Created` from this endpoint in manual testing.

This endpoint is write-capable. It is not permitted during post-import verification.

### Asynchronous Request Status

Endpoint:

```text
GET /api/thirdpartylinker/api/v1/requests/{request_id}
```

Evidence:

- The VB.NET importer polls this path after receiving a request id.
- The Rust importer was corrected to use this path and attach bearer authentication.
- Manual testing confirmed that this is the request-processing status route.

This endpoint confirms request processing status only. It does not return the final stored PLU registry fields required for verification.

## Read APIs Found In Documentation

The PDF documents read operations for some entities, including departments, groups, traceability lists, transactions, and ESL-related data.

These endpoints do not provide enough information to verify imported PLUs because the verifier must compare:

- PLU number
- Department reference
- Group reference
- Product name
- Price
- Barcode information
- Ingredients
- Nutrition facts

Department and group read APIs may help resolve references in a future implementation, but they are not a substitute for a supported PLU readback API.

## PLU Read API Status

The available PLU API documentation does not confirm a read-only PLU lookup endpoint.

The PLU section in the supplied PDF lists these methods for the PLU collection:

```text
POST
PATCH
DELETE
```

No supported `GET` method or read-only lookup path for PLU records was found.

The project must not infer a read path by changing `/write` to `/read`, removing `/write`, adding query parameters, scraping the web UI, or reading the DIGIweb database directly.

## Missing API Information

DIGIweb documentation or a known working request must provide all of the following before operational post-import verification can be implemented:

- Confirmed read-only PLU lookup endpoint path
- HTTP method
- Required authentication and scopes
- Store-selection behavior
- PLU lookup identity rules, including whether department is part of the key
- Request parameters for direct lookup or filtering
- Pagination behavior, if list-based lookup is required
- Whether active, inactive, deleted, or pending PLUs are returned
- Response schema for core PLU fields
- Department representation, including how numeric references relate to UUIDs
- Group representation, including how numeric references relate to UUIDs
- Barcode representation
- Ingredient representation
- Nutrition-fact representation and stable row identity
- Error response behavior for missing PLUs, invalid identifiers, authorization failures, and temporary read-side delay
- Any eventual-consistency guidance after a successful write request

## v0.6.0 Behavior

`to-digi-rs verify-import` performs the source side of verification:

```text
MDB extraction
-> normalization
-> validation
-> first N valid normalized PLUs when --limit is supplied
-> discovery-blocked verification reports
```

It does not:

- Authenticate to DIGIweb
- Call DIGIweb APIs
- Submit PLU write requests
- Delete records
- Access PostgreSQL
- Scrape the browser UI
- Guess undocumented endpoints

The command exits with code `2` while the PLU read API is unconfirmed. This is intentional and means the verification API contract must be obtained before this milestone can proceed to operational field comparison.

## Report Interpretation

When blocked, the reports use:

```text
Verification status: BLOCKED_API_DISCOVERY
```

Selected PLUs are reported as `UNVERIFIED`, not missing or mismatched, because no supported read-only API was available to inspect DIGIweb stored records.

This is different from a future operational verifier:

- `PASS` will require all selected PLUs and required fields to be confirmed.
- `FAIL` will indicate missing, mismatched, or duplicate records.
- `INCOMPLETE` will indicate partial or unavailable supported API coverage.

## Manual Information To Obtain

Ask DIGI or the DIGIweb installation owner for a documented or known-working read-only PLU request and response, including one example for a PLU with barcode, ingredients, and nutrition facts.

At minimum, capture:

```text
HTTP method:
URL path:
Query parameters:
Required headers:
Authentication token source:
Example successful response:
Example missing-PLU response:
Example authorization failure response:
Pagination rules:
Field mapping notes:
```

Do not add network verification code until those details are confirmed.
