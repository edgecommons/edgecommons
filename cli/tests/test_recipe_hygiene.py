"""Repo-wide recipe hygiene: every shipped example + template recipe must be gdk-publish
clean AND model least privilege (no `RequiresPrivilege: true`).

Regression guard for the least-privilege fix: components run as ggc_user — GG IPC, TES-backed
AWS, and the ggc_user-owned work dir all work unprivileged, so example/template recipes must not
default to root. (A real component that needs root sets it deliberately; these exemplars must not.)
"""
import glob
import os

import pytest

from edgecommons_cli.recipe_lint import lint_least_privilege, lint_recipe_text

_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def _shipped_recipes():
    paths = []
    for pattern in ("examples/*/recipe.yaml", "templates/*/recipe.yaml"):
        paths.extend(glob.glob(os.path.join(_REPO_ROOT, pattern)))
    return sorted(paths)


def test_found_recipes():
    # Guard against the glob silently matching nothing (which would make the checks vacuous).
    recipes = _shipped_recipes()
    assert len(recipes) >= 7, f"expected example+template recipes, found {recipes}"


@pytest.mark.parametrize("recipe", _shipped_recipes(), ids=lambda p: os.path.relpath(p, _REPO_ROOT))
def test_recipe_is_least_privilege(recipe):
    with open(recipe, "r", encoding="utf-8") as fh:
        problems = lint_least_privilege(fh.read())
    assert problems == [], f"{recipe} should not require privilege: {problems}"


def test_least_privilege_flags_root():
    # The advisory must actually catch a root recipe.
    assert lint_least_privilege("Manifests:\n  Lifecycle:\n    Run:\n      RequiresPrivilege: true\n")
    assert lint_recipe_text("RequiresPrivilege: true\n") == []  # not a hard gdk problem
