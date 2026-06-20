import os
from typing import Dict, Any

from ggcommons_cli import CommandBase
from ggcommons_cli.recipe_lint import lint_recipe_file


class Validate(CommandBase):
    """Validate a generated component's recipe for `gdk component publish` readiness."""

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "validate",
            "description": "Check a component's recipe for problems that break `gdk component publish`",
            "parameters": [
                {
                    "name": "path",
                    "short": "p",
                    "description": "Path to the component project (or its recipe.yaml)",
                    "type": "string",
                    "required": False,
                    "default": "."
                }
            ]
        }

    def execute_command(self, args: Dict[str, Any]):
        path = args.get("path", ".")
        recipe_path = path if os.path.isfile(path) else os.path.join(path, "recipe.yaml")
        problems = lint_recipe_file(recipe_path)  # raises FileNotFoundError if missing
        if not problems:
            print(f"OK: {recipe_path} has no known GDK-publish issues.")
            return
        print(f"Found {len(problems)} issue(s) in {recipe_path}:")
        for p in problems:
            print(f"  - {p}")
        raise SystemExit(1)
