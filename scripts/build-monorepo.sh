#!/usr/bin/env bash
# Reproducibly assemble the edgecommons monorepo from the individual source repos,
# preserving each repo's full history under its target prefix (git subtree).
#
# Usage:  SRC=/path/to/workspace DEST=/path/to/edgecommons-monorepo ./build-monorepo.sh
# SRC must contain the 11 source repos (siblings); DEST must NOT exist.
set -euo pipefail

SRC="${SRC:?set SRC to the directory holding the source repos}"
DEST="${DEST:?set DEST to the (non-existent) monorepo directory}"
[ -e "$DEST" ] && { echo "DEST already exists: $DEST" >&2; exit 1; }

mkdir -p "$DEST"; cd "$DEST"
git init -q -b main
git commit -q --allow-empty -m "chore: initialize edgecommons monorepo"

imp() { # repo-dir  target-prefix  branch
  git remote add "$1" "$SRC/$1"
  git fetch -q "$1"
  git subtree add -q --prefix="$2" "$1" "$3"   # drop -q / add --squash to taste
  git remote remove "$1"
  echo "imported $1 -> $2 ($3)"
}

imp edgecommons-java-lib        libs/java        main
imp edgecommons-python-lib      libs/python      major-rearch
imp edgecommons-rust-lib        libs/rust        main
imp edgecommons-cli             cli              master
imp edgecommons-test-infra      test-infra       master
imp java-componen-template    templates/java   main
imp python-component-template templates/python main
imp rust-component-template   templates/rust   master
imp java-component-skeleton   examples/java    master
imp python-component-skeleton examples/python  main
imp rust-component-skeleton   examples/rust    main

echo "Done. Now apply the layout path fixes (test-infra + cli) and add the root docs."
