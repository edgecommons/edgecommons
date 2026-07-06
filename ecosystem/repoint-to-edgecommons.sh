#!/usr/bin/env bash
# Repoint edgecommons package coordinates from `mbreissi` to the `edgecommons` org.
# Bash equivalent of repoint-to-edgecommons.ps1 — see ecosystem/RUNBOOK.md (Phase 1b).
#
# Usage:
#   ecosystem/repoint-to-edgecommons.sh            # dry run (lists files)
#   ecosystem/repoint-to-edgecommons.sh --apply    # write changes
#
# SAFE: the rules below do NOT match `com.mbreissi` (Java groupId) or `docs.edgecommons.mbreissi.com`.
# Note: `@mbreissi` -> `@edgecommons` also renames the public-npm addon @edgecommons/streamlog-node;
# edit the sed below to keep it if desired. Run from anywhere inside the repo.
set -euo pipefail

cd "$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
apply="${1:-}"

# Tracked text files that mention an owner-specific ref, excluding generated/doc-mapping files.
mapfile -t files < <(git grep -lE 'github\.com/mbreissi/|ghcr\.io/mbreissi/|@mbreissi' -- . \
    ':!*.lock' ':!package-lock.json' ':!ecosystem/**' ':!docs/ECOSYSTEM.md' || true)

for f in "${files[@]}"; do
    echo "  $f"
    if [ "$apply" = "--apply" ]; then
        sed -i -E \
            -e 's#github\.com/mbreissi/#github.com/edgecommons/#g' \
            -e 's#ghcr\.io/mbreissi/#ghcr.io/edgecommons/#g' \
            -e 's#@mbreissi#@edgecommons#g' \
            "$f"
    fi
done

echo ""
if [ "$apply" = "--apply" ]; then
    echo "APPLIED: rewrote ${#files[@]} file(s). Next: npm install, then rebuild + test all four libs + CLI."
else
    echo "DRY RUN: ${#files[@]} file(s) would change. Re-run with --apply."
fi
