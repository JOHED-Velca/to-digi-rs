# Changelog

## v0.9.0

- Added `to-digi-rs init` to generate a complete deployment directory from the Docker image
- Added the primary `./to-digi` launcher with direct `docker run` execution
- Kept `import.sh` and `run.sh` as compatibility wrappers
- Added `doctor`, `pull`, `resume`, and `version` command paths for deployment use
- Added built-in Starsky profile selection and deployment default profile support
- Added `analyze --raw` for explicit unsanitized analysis
- Added minimal generated configuration with documented defaults
- Added relative token URL resolution against `base_url`
- Added `TO_DIGI_RS_*` environment overrides and client-secret file support
- Improved visible console output for connection checks and command results
- Embedded deployment assets in the binary so init does not require a cloned repository
- Updated deployment packaging and LF shell-script normalization

## v0.8.0

- Added profile-driven PLU sanitization
- Added offline sanitization analysis and reports
- Added safe fill-only rules for configured empty fields
- Added reusable customer TOML profiles
- Added Starsky sanitization profile
- Added sanitized analysis, readiness verification, and import
- Added sanitization profile identity to recovery manifests
- Added profile snapshots for resumable sanitized imports
- Added backward compatibility with v0.7.0 manifests
- Added automatic DIGIweb access-token refresh during import submission and status polling
- Added profile-driven Best Before normalization for DIGIweb selling-date terms
- Preserved the original MDB without modification

## v0.7.0

- Added persistent import-results.json manifests
- Added resumable imports
- Added crash-safe atomic manifest updates
- Added source and payload identity validation
- Added recovery of existing request-status polling
- Added explicit retry for confirmed failed requests
- Added protection against resending unknown submissions
- Added per-PLU attempt history
- Added manifest locking and consistency validation
- Added resume logs and final manifest snapshots

## v0.5.1

- Added a concise analysis summary directly to the terminal
- Added clear department and group prerequisite instructions
- Added source department and group names when available
- Added explicit handling for unavailable source names
- Renamed the deployment runner from run.sh to import.sh
- Retained run.sh as a backward-compatible wrapper
- Preserved detailed text and JSON analysis reports

## v0.5.0

- Expanded offline MDB prerequisite analysis
- Added structured department and group requirements
- Added barcode-format and price-category summaries
- Added source reference-table checks
- Added detailed PluIng matching statistics
- Added ingredient and nutrition availability summaries
- Added structured warning and blocking-error reporting
- Added machine-readable analysis-report.json
- Added installation recommendations
- Allowed analyze to run without DIGIweb credentials
- Reused analysis results in import-readiness verification

## v0.4.0

- Proper command-line interface with `analyze`, `import`, `test-connection`, and `verify`
- `import --limit N`, `import --test`, and `import --continue-on-error`
- Analysis-only report written to `analysis-report.txt`
- Command-specific deployment wrapper behavior and output directories
- Backward-compatible deprecated `[import]` config mapping when no command is supplied
- Deployment bundle, Compose defaults, and publish workflow updated for v0.4.0

## v0.3.0

- One-command importer execution
- Portable deployment directory
- Docker Compose-based runtime
- Automatic bind-mount and host-network handling
- Automatic UID/GID handling
- Timestamped output archiving
- GHCR release-image workflow
- Portable deployment bundle
- Offline image fallback

## v0.2.1

- Accurate status and skip reporting
- Improved batch summaries
- Concise polling logs
- Better backend error extraction
- Real payload-preview files
- Documentation and release polish

## v0.2.0

- First confirmed working full PLU import
- MDB extraction
- Ingredients and nutrition support
- DIGIweb authentication
- Department/group normalization
- Price and barcode mappings
- Asynchronous result polling
