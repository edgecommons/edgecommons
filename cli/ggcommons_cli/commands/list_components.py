"""``ggcommons list-components`` — browse the edgecommons component ecosystem.

Reads the machine-readable component catalog published by the ``edgecommons/registry`` repository and
prints (or emits as JSON) the registered components, optionally filtered by language or category.

This is the discovery half of the ecosystem: ``create-component`` scaffolds new components,
``list-components`` shows what already exists. The catalog lives in a dedicated, public registry repo
so it is readable without authentication.
"""
import json
import os
import urllib.error
import urllib.request
from typing import Any, Dict, List

from ggcommons_cli import CommandBase

# The catalog lives in the dedicated (public) registry repo. Override with --source or
# $GGCOMMONS_REGISTRY_URL (a URL or a local path).
DEFAULT_REGISTRY_URL = (
    "https://raw.githubusercontent.com/edgecommons/registry/main/components.json"
)


def _load_catalog(source: str) -> Dict[str, Any]:
    """Load the catalog from an http(s) URL or a local file path.

    Raises ``RuntimeError`` with a clean message on any fetch/parse/shape error.
    """
    if source.startswith(("http://", "https://")):
        try:
            with urllib.request.urlopen(source, timeout=15) as resp:  # noqa: S310 (trusted URL)
                raw = resp.read().decode("utf-8")
        except urllib.error.HTTPError as exc:
            raise RuntimeError(
                f"could not fetch the component registry ({exc.code} {exc.reason}) from {source}"
            ) from exc
        except urllib.error.URLError as exc:
            raise RuntimeError(
                f"could not reach the component registry at {source}: {exc.reason}"
            ) from exc
    else:
        if not os.path.isfile(source):
            raise RuntimeError(f"registry source not found: {source}")
        with open(source, "r", encoding="utf-8") as fh:
            raw = fh.read()

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"registry is not valid JSON: {exc}") from exc

    if not isinstance(data, dict) or not isinstance(data.get("components"), list):
        raise RuntimeError(
            "registry is malformed: expected a JSON object with a 'components' array"
        )
    return data


def _filter(components: List[Dict[str, Any]], language: str, category: str) -> List[Dict[str, Any]]:
    """Filter entries by language and/or category (case-insensitive); empty filters match all."""
    out = []
    for c in components:
        if language and str(c.get("language", "")).lower() != language.lower():
            continue
        if category and str(c.get("category", "")).lower() != category.lower():
            continue
        out.append(c)
    return out


class ListComponents(CommandBase):
    """List the components registered in the edgecommons ecosystem registry."""

    @classmethod
    def get_json_configuration(cls):
        return {
            "name": "list-components",
            "description": "List components registered in the edgecommons ecosystem registry",
            "parameters": [
                {
                    "name": "source",
                    "description": (
                        "Registry URL or local components.json path "
                        "(default: the edgecommons/registry catalog, or $GGCOMMONS_REGISTRY_URL)"
                    ),
                    "type": "string",
                },
                {
                    "name": "language",
                    "description": "Filter by language (JAVA|PYTHON|RUST|TYPESCRIPT)",
                    "type": "string",
                },
                {
                    "name": "category",
                    "description": "Filter by category (adapter|processor|sink)",
                    "type": "string",
                },
                {
                    "name": "json",
                    "description": "Emit the matching catalog entries as raw JSON",
                    "type": "boolean",
                },
            ],
        }

    def execute_command(self, args: Dict[str, Any]):
        source = (
            args.get("source")
            or os.environ.get("GGCOMMONS_REGISTRY_URL")
            or DEFAULT_REGISTRY_URL
        )
        catalog = _load_catalog(source)
        components = _filter(
            catalog.get("components", []), args.get("language"), args.get("category")
        )

        if args.get("json"):
            print(json.dumps(components, indent=2))
            return

        if not components:
            print(f"No components matched (registry: {source}).")
            return

        name_w = max(len(c.get("name", "")) for c in components)
        lang_w = max(len(str(c.get("language", ""))) for c in components)
        cat_w = max(len(str(c.get("category", ""))) for c in components)

        print(f"edgecommons components ({len(components)}) - source: {source}\n")
        for c in sorted(components, key=lambda x: (x.get("category", ""), x.get("name", ""))):
            print(
                f"  {c.get('name', '').ljust(name_w)}  "
                f"{str(c.get('language', '')).ljust(lang_w)}  "
                f"{str(c.get('category', '')).ljust(cat_w)}  "
                f"{c.get('description', '')}"
            )
            repo = c.get("repo")
            if repo:
                pad = " " * (name_w + lang_w + cat_w + 6)
                print(f"  {pad}https://github.com/{repo}")
