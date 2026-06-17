"""Shared Greengrass recipe linting — flags constructs that break `gdk component publish`."""
import os
import re
from typing import List


def lint_recipe_text(recipe: str) -> List[str]:
    """Return a list of human-readable problems found in a recipe's text."""
    problems = []
    if "{COMPONENT_NAME}" in recipe:
        problems.append(
            "ComponentName uses the '{COMPONENT_NAME}' placeholder; GDK does not substitute "
            "it and `gdk component publish` rejects the recipe. Use the literal component name."
        )
    if re.search(r'^\s*Permissions:', recipe, re.MULTILINE):
        problems.append(
            "An artifact 'Permissions:' block is present; CreateComponentVersion rejects it. "
            "Remove it and make artifacts executable via an Install lifecycle (chmod)."
        )
    if "<<" in recipe and ">>" in recipe:
        problems.append("Unsubstituted '<<...>>' placeholder(s) remain in the recipe.")
    return problems


def lint_recipe_file(recipe_path: str) -> List[str]:
    """Lint a recipe file; raises FileNotFoundError if it does not exist."""
    if not os.path.isfile(recipe_path):
        raise FileNotFoundError(f"Recipe not found: {recipe_path}")
    with open(recipe_path, 'r', encoding='utf-8') as fh:
        return lint_recipe_text(fh.read())
