#!/usr/bin/env bash
# Propagate the canonical EdgeCommons config schema into each language library.
#
# The canonical schema lives at schema/edgecommons-config-schema.json and is the SINGLE
# SOURCE OF TRUTH. Each lib embeds/loads its own copy (cargo include_str!, tsc import,
# Python package-data, Java classpath resource), so the canonical file must be copied in.
#
#   ./schema/sync-schema.sh            # copy canonical -> all per-lib copies
#   ./schema/sync-schema.sh --check    # verify copies match (CI drift gate; nonzero on drift)
#
# Works under Git Bash on Windows and bash on Linux CI.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CANON="$ROOT/schema/edgecommons-config-schema.json"

# canonical -> destination copies (TS uses a different filename)
TARGETS=(
  "$ROOT/libs/rust/resources/edgecommons-config-schema.json"
  "$ROOT/libs/ts/src/config/schema.json"
  "$ROOT/libs/python/edgecommons/resources/edgecommons-config-schema.json"
  "$ROOT/libs/java/src/main/resources/edgecommons-config-schema.json"
  "$ROOT/libs/java/doc/edgecommons-config-schema.json"
)

if [[ ! -f "$CANON" ]]; then
  echo "ERROR: canonical schema not found: $CANON" >&2
  exit 2
fi

CHECK=0
[[ "${1:-}" == "--check" ]] && CHECK=1

drift=0
for dst in "${TARGETS[@]}"; do
  rel="${dst#"$ROOT/"}"
  if [[ $CHECK -eq 1 ]]; then
    if [[ ! -f "$dst" ]] || ! cmp -s "$CANON" "$dst"; then
      echo "DRIFT: $rel differs from canonical schema"
      drift=1
    else
      echo "ok:    $rel"
    fi
  else
    mkdir -p "$(dirname "$dst")"
    cp "$CANON" "$dst"
    echo "synced: $rel"
  fi
done

if [[ $CHECK -eq 1 && $drift -eq 1 ]]; then
  echo "" >&2
  echo "Config schema copies are out of sync with schema/edgecommons-config-schema.json." >&2
  echo "Run ./schema/sync-schema.sh and commit the result." >&2
  exit 1
fi

[[ $CHECK -eq 1 ]] && echo "All schema copies match the canonical source." || echo "Done."
