#!/usr/bin/env bash
set -u

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)" || exit 2

printf 'NOTICE: run.sh has been renamed to import.sh.\n' >&2
printf 'Forwarding this command for backward compatibility.\n' >&2

if [ ! -x "$SCRIPT_DIR/import.sh" ]; then
    printf 'ERROR: import.sh was not found beside run.sh or is not executable.\n' >&2
    exit 2
fi

"$SCRIPT_DIR/import.sh" "$@"
exit $?
