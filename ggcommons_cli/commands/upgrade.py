import os
import re
from typing import Dict, Any, List

from ggcommons_cli import CommandBase


class Upgrade(CommandBase):
    """Bump a generated component's ggcommons dependency to a specific version.

    Updates whichever dependency manifest is present:
      - Java:   pom.xml             (<artifactId>ggcommons</artifactId> version)
      - Python: requirements.txt    (greengrass-commons pin)
      - Rust:   Cargo.toml          (ggcommons version dependency; path deps are left as-is)
    """

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "upgrade",
            "description": "Update a component's ggcommons dependency to a given version",
            "parameters": [
                {
                    "name": "path",
                    "short": "p",
                    "description": "Path to the component project",
                    "type": "string",
                    "required": False,
                    "default": "."
                },
                {
                    "name": "to",
                    "short": "t",
                    "description": "Target ggcommons version (e.g. 1.2.3)",
                    "type": "string",
                    "required": True
                }
            ]
        }

    def execute_command(self, args: Dict[str, Any]):
        path = args.get("path", ".")
        version = args.get("to")
        if not version:
            raise ValueError("A target version is required (-t/--to).")
        if not os.path.isdir(path):
            raise FileNotFoundError(f"Project directory not found: {path}")

        changes: List[str] = []
        changes += self._bump_pom(os.path.join(path, "pom.xml"), version)
        changes += self._bump_requirements(os.path.join(path, "requirements.txt"), version)
        changes += self._bump_cargo(os.path.join(path, "Cargo.toml"), version)

        if not changes:
            print("No ggcommons dependency found to upgrade.")
            return
        for c in changes:
            print(c)

    @staticmethod
    def _bump_pom(pom: str, version: str) -> List[str]:
        if not os.path.isfile(pom):
            return []
        with open(pom, 'r', encoding='utf-8') as fh:
            text = fh.read()
        pattern = re.compile(r'(<artifactId>ggcommons</artifactId>\s*<version>)[^<]+(</version>)')
        new, n = pattern.subn(rf'\g<1>{version}\g<2>', text)
        if not n:
            return ["pom.xml: no ggcommons <version> found to update."]
        with open(pom, 'w', encoding='utf-8') as fh:
            fh.write(new)
        return [f"pom.xml: ggcommons -> {version}"]

    @staticmethod
    def _bump_requirements(req: str, version: str) -> List[str]:
        if not os.path.isfile(req):
            return []
        with open(req, 'r', encoding='utf-8') as fh:
            lines = fh.read().splitlines()
        changed = False
        for i, line in enumerate(lines):
            if re.match(r'^\s*greengrass-commons\b', line):
                lines[i] = f"greengrass-commons=={version}"
                changed = True
        if not changed:
            return ["requirements.txt: no greengrass-commons entry found."]
        with open(req, 'w', encoding='utf-8') as fh:
            fh.write("\n".join(lines) + "\n")
        return [f"requirements.txt: greengrass-commons=={version}"]

    @staticmethod
    def _bump_cargo(cargo: str, version: str) -> List[str]:
        if not os.path.isfile(cargo):
            return []
        with open(cargo, 'r', encoding='utf-8') as fh:
            text = fh.read()
        if re.search(r'^ggcommons\s*=\s*\{[^}]*\bpath\s*=', text, re.MULTILINE):
            return ["Cargo.toml: ggcommons is a path dependency; nothing to version-bump."]
        # `ggcommons = "x"`
        new, n = re.subn(r'(^ggcommons\s*=\s*")[^"]+(")', rf'\g<1>{version}\g<2>', text, flags=re.MULTILINE)
        if not n:
            # `ggcommons = { version = "x", ... }`
            new, n = re.subn(r'(^ggcommons\s*=\s*\{[^}]*version\s*=\s*")[^"]+(")',
                             rf'\g<1>{version}\g<2>', text, flags=re.MULTILINE)
        if not n:
            return ["Cargo.toml: no ggcommons version dependency found."]
        with open(cargo, 'w', encoding='utf-8') as fh:
            fh.write(new)
        return [f"Cargo.toml: ggcommons -> {version}"]
