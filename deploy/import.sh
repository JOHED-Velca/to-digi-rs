#!/usr/bin/env bash
set -u

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)" || exit 2

if [ ! -x "$SCRIPT_DIR/to-digi" ]; then
    printf 'ERROR: to-digi was not found beside import.sh or is not executable.\n' >&2
    printf 'Run the v0.9.0 init command again to regenerate deployment launchers.\n' >&2
    exit 2
fi

printf 'NOTICE: import.sh is a compatibility wrapper. Use ./to-digi for new deployments.\n' >&2
"$SCRIPT_DIR/to-digi" "$@"
exit $?
