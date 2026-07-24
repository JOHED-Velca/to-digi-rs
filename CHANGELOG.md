# Changelog

## v0.6.0

- Added the `verify-import` command for post-import verification API discovery
- Added `verify-import --limit N` selection after source normalization and validation
- Added text and JSON verification discovery reports
- Added documented DIGIweb read-API evidence and missing contract details
- Added read-only safety confirmations for blocked verification
- Preserved the existing pre-import `verify` command
- Bumped deployment scripts, package metadata, and image examples for v0.6.0

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
