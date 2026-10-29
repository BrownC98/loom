#!/usr/bin/env bash
set -u
quick=; [ "${1:-}" = --quick ] && { quick=--quick; shift; }
doc=$(realpath "$1") || exit 1

[ -n "$quick" ] || loom show "$doc" || exit 1

timeout 300 node "$(dirname "$0")/check-callouts.cjs" "$doc" $quick || { echo "show: document check failed (see the overlaps above)" >&2; exit 1; }

[ -n "$quick" ] && { echo "show: quick check at 1440 passed. Run show.sh without --quick before loom ask --doc." >&2; exit 0; }
loom show --checked "$doc" || exit 1
