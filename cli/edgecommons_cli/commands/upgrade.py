import json
import os
import re
from typing import Dict, Any, List

from edgecommons_cli import CommandBase


class Upgrade(CommandBase):
    """Bump a generated component's edgecommons dependency to a specific version.

    Updates whichever dependency manifest is present:
      - Java:       pom.xml             (<artifactId>edgecommons</artifactId> version)
      - Python:     requirements.txt    (edgecommons pin)
      - Rust:       Cargo.toml          (edgecommons version dependency; path deps are left as-is)
      - TypeScript: package.json        (edgecommons dependency; `file:` path deps are left as-is)
    """

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "upgrade",
            "description": "Update a component's edgecommons dependency to a given version",
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
                    "description": "Target edgecommons version (e.g. 1.2.3)",
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
        changes += self._bump_package_json(os.path.join(path, "package.json"), version)

        if not changes:
            print("No edgecommons dependency found to upgrade.")
            return
        for c in changes:
            print(c)

    @staticmethod
    def _bump_pom(pom: str, version: str) -> List[str]:
        if not os.path.isfile(pom):
            return []
        with open(pom, 'r', encoding='utf-8') as fh:
            text = fh.read()
        pattern = re.compile(r'(<artifactId>edgecommons</artifactId>\s*<version>)[^<]+(</version>)')
        new, n = pattern.subn(rf'\g<1>{version}\g<2>', text)
        if not n:
            return ["pom.xml: no edgecommons <version> found to update."]
        with open(pom, 'w', encoding='utf-8') as fh:
            fh.write(new)
        return [f"pom.xml: edgecommons -> {version}"]

    @staticmethod
    def _bump_requirements(req: str, version: str) -> List[str]:
        if not os.path.isfile(req):
            return []
        with open(req, 'r', encoding='utf-8') as fh:
            lines = fh.read().splitlines()
        changed = False
        for i, line in enumerate(lines):
            if re.match(r'^\s*edgecommons\b', line):
                lines[i] = f"edgecommons=={version}"
                changed = True
        if not changed:
            return ["requirements.txt: no edgecommons entry found."]
        with open(req, 'w', encoding='utf-8') as fh:
            fh.write("\n".join(lines) + "\n")
        return [f"requirements.txt: edgecommons=={version}"]

    @staticmethod
    def _bump_cargo(cargo: str, version: str) -> List[str]:
        if not os.path.isfile(cargo):
            return []
        with open(cargo, 'r', encoding='utf-8') as fh:
            text = fh.read()
        if re.search(r'^edgecommons\s*=\s*\{[^}]*\bpath\s*=', text, re.MULTILINE):
            return ["Cargo.toml: edgecommons is a path dependency; nothing to version-bump."]
        # `edgecommons = "x"`
        new, n = re.subn(r'(^edgecommons\s*=\s*")[^"]+(")', rf'\g<1>{version}\g<2>', text, flags=re.MULTILINE)
        if not n:
            # `edgecommons = { version = "x", ... }`
            new, n = re.subn(r'(^edgecommons\s*=\s*\{[^}]*version\s*=\s*")[^"]+(")',
                             rf'\g<1>{version}\g<2>', text, flags=re.MULTILINE)
        if not n:
            return ["Cargo.toml: no edgecommons version dependency found."]
        with open(cargo, 'w', encoding='utf-8') as fh:
            fh.write(new)
        return [f"Cargo.toml: edgecommons -> {version}"]

    @staticmethod
    def _bump_package_json(pkg: str, version: str) -> List[str]:
        if not os.path.isfile(pkg):
            return []
        with open(pkg, 'r', encoding='utf-8') as fh:
            data = json.load(fh)
        updated = False
        for section in ("dependencies", "devDependencies"):
            deps = data.get(section)
            if not isinstance(deps, dict) or "edgecommons" not in deps:
                continue
            current = deps["edgecommons"]
            # Leave `file:`/`link:` path dependencies as-is (mirrors Cargo path deps).
            if isinstance(current, str) and (current.startswith("file:") or current.startswith("link:")):
                return ["package.json: edgecommons is a path dependency; nothing to version-bump."]
            deps["edgecommons"] = version
            updated = True
        if not updated:
            return ["package.json: no edgecommons dependency found."]
        with open(pkg, 'w', encoding='utf-8') as fh:
            json.dump(data, fh, indent=2)
            fh.write("\n")
        return [f"package.json: edgecommons -> {version}"]
