#!/usr/bin/env bash
# LOCAL DEV ONLY — the Python analog of the bridge's gitignored .cargo/config.toml `[patch]`
# override and the edge-console's `link-sibling-lib.mjs` workspace stub: pip has no "path
# dependency" concept, so the local-dev equivalent is an EDITABLE install of the SIBLING
# edgecommons Python checkout (../core/libs/python, relative to this component's org-level
# checkout — see the edgecommons org CLAUDE.md "every repo is a sibling"). `pip install -e`
# always wins over whatever `pip install -r requirements.txt` resolved (regardless of install
# order), so it's safe to run this before or after installing requirements.txt.
#
# Usage:  bash scripts/link-sibling-lib.sh   (then re-run your test/run command — no reinstall
#         of requirements.txt needed unless another dependency changed)
#
# TODO(release): once edgecommons tags a python-lib/vX.Y.Z release off this branch, requirements.txt's
# bare `edgecommons` should become a pinned coordinate — see modbus-adapter/requirements.txt
# for the `edgecommons @ git+https://github.com/edgecommons/edgecommons.git@python-lib/vX.Y.Z
# #subdirectory=libs/python` form this project is expected to adopt at that point.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# HERE = <this component>/scripts; one .. = the component root, two = the org root
# (edgecommons/, where every repo including edgecommons is a sibling checkout) - see the
# edgecommons org CLAUDE.md.
SIBLING="$(cd "$HERE/../../core/libs/python" && pwd)"
echo "Installing sibling edgecommons (editable) from: $SIBLING"
pip install -e "$SIBLING"
