"""``edgecommons list-components`` — browse the edgecommons component ecosystem.

Reads the machine-readable component catalog from the ``edgecommons/registry`` repository and prints
(or emits as JSON) the registered components, optionally filtered by language or category.

This is the discovery half of the ecosystem: ``create-component`` scaffolds new components,
``list-components`` shows what already exists. The registry repo is **private**, so by default the
catalog is fetched with authentication via the GitHub CLI (``gh``). Override the source with
``--source`` / ``$EDGECOMMONS_REGISTRY_URL`` (a URL — e.g. if you make the registry public — or a local
``components.json`` path), which bypasses ``gh`` entirely.
"""
import json
import os
import subprocess
import urllib.error
import urllib.request
from typing import Any, Dict, List

from edgecommons_cli import CommandBase

# Default: the private registry, read via `gh api` (authenticated, no token wrangling here).
DEFAULT_REGISTRY_REPO = "edgecommons/registry"
DEFAULT_REGISTRY_PATH = "components.json"
DEFAULT_REGISTRY_REF = "main"


def _load_text_via_gh(repo: str, path: str, ref: str) -> str:
    """Fetch a file's raw content from a (private) GitHub repo via the GitHub CLI."""
    endpoint = f"repos/{repo}/contents/{path}"
    if ref:
        endpoint += f"?ref={ref}"
    try:
        proc = subprocess.run(
            ["gh", "api", endpoint, "-H", "Accept: application/vnd.github.raw"],
            capture_output=True,
            text=True,
            timeout=30,
        )
    except FileNotFoundError as exc:
        raise RuntimeError(
            "the GitHub CLI ('gh') is required to read the private registry; install it and run "
            "'gh auth login', or pass --source <url|path>"
        ) from exc
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip()
        raise RuntimeError(f"could not read the registry {repo}/{path} via gh api: {detail}")
    return proc.stdout


def _load_text_from_url(url: str) -> str:
    """Fetch raw text from an http(s) URL (for a public registry or an explicit override)."""
    try:
        with urllib.request.urlopen(url, timeout=15) as resp:  # noqa: S310 (caller-supplied URL)
            return resp.read().decode("utf-8")
    except urllib.error.HTTPError as exc:
        raise RuntimeError(
            f"could not fetch the component registry ({exc.code} {exc.reason}) from {url}"
        ) from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"could not reach the component registry at {url}: {exc.reason}") from exc


def _load_text_from_file(path: str) -> str:
    if not os.path.isfile(path):
        raise RuntimeError(f"registry source not found: {path}")
    with open(path, "r", encoding="utf-8") as fh:
        return fh.read()


def _load_catalog(source: str = None) -> Dict[str, Any]:
    """Load + validate the catalog.

    ``source`` resolution: an http(s) URL → fetched over HTTP; any other string → a local file path;
    ``None`` → the default private registry via ``gh``. Raises ``RuntimeError`` with a clean message
    on any fetch/parse/shape error.
    """
    if not source:
        raw = _load_text_via_gh(DEFAULT_REGISTRY_REPO, DEFAULT_REGISTRY_PATH, DEFAULT_REGISTRY_REF)
    elif source.startswith(("http://", "https://")):
        raw = _load_text_from_url(source)
    else:
        raw = _load_text_from_file(source)

    try:
        data = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"registry is not valid JSON: {exc}") from exc

    if not isinstance(data, dict) or not isinstance(data.get("components"), list):
        raise RuntimeError("registry is malformed: expected a JSON object with a 'components' array")
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
                        "Registry URL or local components.json path (default: the private "
                        "edgecommons/registry catalog via gh, or $EDGECOMMONS_REGISTRY_URL)"
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
        source = args.get("source") or os.environ.get("EDGECOMMONS_REGISTRY_URL")
        catalog = _load_catalog(source)
        components = _filter(
            catalog.get("components", []), args.get("language"), args.get("category")
        )

        label = source or f"gh:{DEFAULT_REGISTRY_REPO}/{DEFAULT_REGISTRY_PATH}"

        if args.get("json"):
            print(json.dumps(components, indent=2))
            return

        if not components:
            print(f"No components matched (registry: {label}).")
            return

        name_w = max(len(c.get("name", "")) for c in components)
        lang_w = max(len(str(c.get("language", ""))) for c in components)
        cat_w = max(len(str(c.get("category", ""))) for c in components)

        print(f"edgecommons components ({len(components)}) - source: {label}\n")
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
