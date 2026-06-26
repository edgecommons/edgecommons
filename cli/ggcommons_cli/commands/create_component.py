import json
import os
import re
import shutil
import sys
import tempfile

from git import Repo, GitCommandError

from ggcommons_cli import CommandBase
from typing import Dict, Any, Optional

# A template may ship this manifest to drive generation generically (no CLI code
# changes needed to add a language). See _apply_manifest for the schema.
MANIFEST_NAME = "ggcommons-template.json"

# Monorepo root, resolved relative to this file (cli/ggcommons_cli/commands -> repo root).
_REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))


def _bundled_templates_root():
    """templates/ bundled inside the installed package (built into the wheel by
    setup.py's build_py), or None when running from a source checkout."""
    try:
        from importlib.resources import files
        path = os.path.join(str(files("ggcommons_cli")), "templates")
        return path if os.path.isdir(path) else None
    except Exception:
        return None


def _default_template_source(language: str) -> str:
    """Default template source for a language: the bundled copy when the CLI is
    installed (works offline), else the in-repo templates/<lang> for dev/checkout."""
    bundled = _bundled_templates_root()
    if bundled:
        candidate = os.path.join(bundled, language)
        if os.path.isdir(candidate):
            return candidate
    return os.path.join(_REPO_ROOT, "templates", language)


# Default template sources per language. Override any with --template-url (a git URL
# cloned, or a local directory copied — see _fetch_template). Templates are bundled
# into the wheel at build time, so the installed CLI scaffolds offline.
DEFAULT_TEMPLATE_SOURCES = {
    "JAVA": _default_template_source("java"),
    "PYTHON": _default_template_source("python"),
    "RUST": _default_template_source("rust"),
    "TYPESCRIPT": _default_template_source("typescript"),
}

# Languages whose components depend on the ggcommons library via a path dependency
# (Cargo path dep for Rust, npm `file:` dep for TypeScript). These require a valid
# --ggcommons-path and a language-specific default (libs/<lang> in this monorepo).
_GGCOMMONS_PATH_DEFAULTS = {
    "RUST": ("libs", "rust"),
    "TYPESCRIPT": ("libs", "ts"),
}

# Platform targets a generated component can be built/deployed for. These drive which
# OPTIONAL, platform-specific artifacts a template emits — e.g. the Kubernetes Dockerfile
# + k8s/ manifests are emitted only when KUBERNETES is a selected target — via the
# manifest's "conditional" section (see _apply_manifest). Default = all three, so the
# non-interactive path stays backward-compatible (emit everything).
VALID_PLATFORMS = ("GREENGRASS", "HOST", "KUBERNETES")

# Where a generated component resolves the ggcommons library from. `local` = a path/file
# dependency on a monorepo checkout (Rust/TS today); `registry` = the published artifact
# (GitHub Packages / git-tag / registry, per docs/platform/DESIGN-packaging.md §13). Drives
# the manifest "conditional" flag `dep:<source>` so a template can ship both dep forms and
# emit only the chosen one.
VALID_DEP_SOURCES = ("registry", "local")
_DEFAULT_DEP_SOURCE = "local"

# Coordinates for the `registry` dependency source (DESIGN-packaging §13). Initial version;
# `ggcommons upgrade --to <ver>` bumps a generated component later.
_GGCOMMONS_VERSION = "0.1.0"
_GGCOMMONS_GIT_URL = "https://github.com/mbreissi/ggcommons"


class CreateComponent(CommandBase):

    def __init__(self):
        super().__init__()
        self.component_full_name = None
        self.component_name = None
        self.component_description = None
        self.component_language = None
        self.project_path = None
        self.jar_name = None
        self.package_name = None
        self.bin_name = None
        self.author = None
        self.bucket = None
        self.region = None
        self.ggcommons_path = None
        self.template_url = None
        self.template_ref = None
        self.force = False
        self.interactive = False
        self.platforms = set(VALID_PLATFORMS)
        self.dep_source = _DEFAULT_DEP_SOURCE

    @classmethod
    def _default_ggcommons_path(cls, language: Optional[str]) -> str:
        """Default ggcommons library path for a path-dependency language: libs/<lang>
        in this monorepo, as an absolute, forward-slash path (Cargo/npm-friendly).
        Falls back to libs/rust when the language is unknown (e.g. argparse default)."""
        parts = _GGCOMMONS_PATH_DEFAULTS.get(language or "", ("libs", "rust"))
        return os.path.join(_REPO_ROOT, *parts).replace("\\", "/")

    @classmethod
    def get_json_configuration(cls):
        # Static default for argparse (language not yet known): libs/rust. The real
        # default is re-resolved per language in execute_command (see _default_ggcommons_path).
        default_ggcommons_path = cls._default_ggcommons_path(None)
        return {
            "name": "create-component",
            "description": "Create a new component that uses ggcommons",
            "parameters": [
                {
                    "name": "name",
                    "short": "n",
                    "description": "Fully qualified name of the component "
                                   "(prompted in --interactive mode if omitted)",
                    "type": "string",
                    "required": False
                },
                {
                    "name": "description",
                    "short": "d",
                    "description": "Short description for the component",
                    "type": "string",
                    "required": False,
                    "default": "This is a Greengrass v2 component"
                },
                {
                    "name": "language",
                    "short": "l",
                    "description": "Programming language "
                                   "(prompted in --interactive mode if omitted)",
                    "type": "string",
                    "required": False,
                    "enum": sorted(DEFAULT_TEMPLATE_SOURCES.keys())
                },
                {
                    "name": "path",
                    "short": "p",
                    "description": "Path to the directory where the component will be created",
                    "type": "string",
                    "required": False,
                    "default": "."
                },
                {
                    "name": "jar",
                    "short": "j",
                    "description": "Name of the jar file (Java)",
                    "type": "string",
                    "required": False
                },
                {
                    "name": "author",
                    "short": "a",
                    "description": "Author of the component",
                    "type": "string",
                    "required": False,
                    "default": "Amazon Web Services"
                },
                {
                    "name": "bucket",
                    "short": "b",
                    "description": "S3 bucket to store the component",
                    "type": "string",
                    "required": False,
                    "default": "greengrass-component-artifacts-us-east-1"
                },
                {
                    "name": "region",
                    "short": "r",
                    "description": "AWS region",
                    "type": "string",
                    "required": False,
                    "default": "us-east-1"
                },
                {
                    "name": "ggcommons-path",
                    "short": "g",
                    "description": "Absolute path to the ggcommons library (RUST/TYPESCRIPT "
                                   "only; becomes the Cargo / npm `file:` path dependency)",
                    "type": "string",
                    "required": False,
                    "default": default_ggcommons_path
                },
                {
                    "name": "template-url",
                    "short": "u",
                    "description": "Override the template source: a git URL or a local "
                                   "directory path (advanced; for a local or forked template)",
                    "type": "string",
                    "required": False
                },
                {
                    "name": "template-ref",
                    "description": "Git branch or tag to clone for the template "
                                   "(default: the repo's default branch; ignored for local dirs)",
                    "type": "string",
                    "required": False
                },
                {
                    "name": "force",
                    "short": "f",
                    "description": "Overwrite the target directory if it already exists",
                    "type": "boolean",
                    "required": False
                },
                {
                    "name": "interactive",
                    "short": "i",
                    "description": "Prompt for the inputs interactively (a guided wizard). "
                                   "Auto-enabled when -n/--name is omitted on a terminal.",
                    "type": "boolean",
                    "required": False
                },
                {
                    "name": "platforms",
                    "description": "Comma-separated target platforms the component will be "
                                   "built/deployed for (GREENGRASS,HOST,KUBERNETES). Controls "
                                   "which platform-specific artifacts are emitted (e.g. the k8s "
                                   "Dockerfile + manifests only when KUBERNETES is included). "
                                   "Default: all.",
                    "type": "string",
                    "required": False,
                    "default": ",".join(VALID_PLATFORMS)
                },
                {
                    "name": "dep-source",
                    "description": "How the generated component depends on the ggcommons library: "
                                   "'local' (a path/file dependency on a monorepo checkout) or "
                                   "'registry' (the published artifact).",
                    "type": "string",
                    "required": False,
                    "default": _DEFAULT_DEP_SOURCE,
                    "enum": list(VALID_DEP_SOURCES)
                }
            ]
        }

    def execute_command(self, args: Dict[str, Any]):
        # Interactive wizard: explicit -i/--interactive, or no -n/--name on a TTY. The wizard
        # fills the args dict, then the normal (non-interactive) flow runs unchanged.
        self.interactive = bool(args.get('interactive')) or (
            not args.get('name') and sys.stdin.isatty()
        )
        if self.interactive:
            args = self._run_wizard(dict(args))

        self.component_full_name = args.get('name', "ComponentSkeleton")
        self.component_name = self.component_full_name.split('.')[-1]
        self.package_name = self.component_full_name.lower()
        # Cargo crate/binary name: lowercase, non-alphanumerics collapsed to hyphens.
        self.bin_name = re.sub(r'[^a-z0-9]+', '-', self.component_name.lower()).strip('-') or "component"
        self.component_description = args.get('description')
        self.component_language = args.get('language')
        self.project_path = args.get('path', ".")
        # Default the jar name (Java) to the component name rather than emitting "None".
        self.jar_name = args.get('jar') or self.component_name
        self.author = args.get('author')
        self.bucket = args.get('bucket')
        self.region = args.get('region')
        # argparse maps --ggcommons-path / --template-url to underscore dest names.
        self.ggcommons_path = args.get('ggcommons_path')
        # The argparse default is the static libs/rust path (language isn't known when
        # the parser is built). When the user accepted that default but chose a
        # path-dependency language with a different default (e.g. TYPESCRIPT -> libs/ts),
        # re-resolve to the language-specific default.
        static_default = self._default_ggcommons_path(None)
        if self.ggcommons_path == static_default and self.component_language in _GGCOMMONS_PATH_DEFAULTS:
            self.ggcommons_path = self._default_ggcommons_path(self.component_language)
        self.template_url = args.get('template_url')
        self.template_ref = args.get('template_ref')
        self.force = bool(args.get('force'))
        self.platforms = self._parse_platforms(args.get('platforms'))
        self.dep_source = (args.get('dep_source') or _DEFAULT_DEP_SOURCE).lower()

        self._validate_args()
        target_dir = self._target_dir()
        self._guard_target_dir(target_dir)

        source = self.template_url or DEFAULT_TEMPLATE_SOURCES.get(self.component_language)
        if not source:
            raise ValueError(
                f"No template source registered for language '{self.component_language}'. "
                f"Pass --template-url."
            )

        print(f"Generating {self.component_language} Greengrass component "
              f"{self.component_name} ({self.component_full_name})")
        self._fetch_template(source, target_dir)

        manifest = self._read_manifest(target_dir)
        if manifest is None:
            raise RuntimeError(
                f"Template for '{self.component_language}' has no '{MANIFEST_NAME}'. "
                f"Add a manifest to the template (see an existing template for the schema)."
            )
        # Manifest-driven (generic) — no per-language CLI code required.
        self._apply_manifest(manifest, target_dir, self._placeholder_values())

        # Post-generation correctness checks (fail fast on a broken scaffold).
        self._verify_no_leftover_tokens(target_dir)
        self._lint_recipe(os.path.join(target_dir, "recipe.yaml"))
        print(f"Done. Component generated at: {target_dir}")

    # ----- inputs / placeholder values -------------------------------------------------

    def _target_dir(self) -> str:
        return str(os.path.join(self.project_path, self.component_name))

    def _placeholder_values(self) -> Dict[str, str]:
        """The single source of truth mapping placeholder name -> value."""
        return {
            "COMPONENTFULLNAME": self.component_full_name,
            "COMPONENTNAME": self.component_name,
            "PACKAGE": self.package_name,
            "PACKAGEPATH": self.package_name.replace(".", "/"),
            "MAINCLASSNAME": f"{self.package_name}.{self.component_name}",
            "JARNAME": self.jar_name,
            "BINNAME": self.bin_name,
            "DESCRIPTION": self.component_description or "",
            "AUTHOR": self.author or "",
            "BUCKET": self.bucket or "",
            "REGION": self.region or "",
            "GGCOMMONS_PATH": self.ggcommons_path or "",
            "GGCOMMONS_DEP": self._ggcommons_dep(),
        }

    def _ggcommons_dep(self) -> str:
        """The dependency-declaration fragment a path-dep template (Rust/TS) substitutes for
        ``<<GGCOMMONS_DEP>>``, chosen by --dep-source. `local` = a path/file dependency on the
        monorepo checkout (the dev default); `registry` = the published artifact (git tag for
        Rust, a semver range for TS GitHub-Packages npm). See DESIGN-packaging §13."""
        lang = self.component_language
        if self.dep_source == "registry":
            if lang == "RUST":
                return f'git = "{_GGCOMMONS_GIT_URL}", tag = "rust-lib/v{_GGCOMMONS_VERSION}"'
            if lang == "TYPESCRIPT":
                return f"^{_GGCOMMONS_VERSION}"
            return ""
        path = (self.ggcommons_path or "").replace("\\", "/")
        if lang == "RUST":
            return f'path = "{path}"'
        if lang == "TYPESCRIPT":
            return f"file:{path}"
        return ""

    # ----- platforms / conditional generation ------------------------------------------

    @staticmethod
    def _parse_platforms(raw) -> set:
        """Parse the platforms input (a comma-separated string, or an iterable) into an
        upper-cased set. Empty/absent -> all platforms (backward-compatible default)."""
        if not raw:
            return set(VALID_PLATFORMS)
        items = raw if isinstance(raw, (set, list, tuple)) else str(raw).split(",")
        out = {str(p).strip().upper() for p in items if str(p).strip()}
        return out or set(VALID_PLATFORMS)

    def _condition_flags(self) -> set:
        """The active condition flags a template manifest's "conditional" entries test
        against: one ``platform:<P>`` per selected platform + ``dep:<source>``."""
        flags = {f"platform:{p}" for p in self.platforms}
        flags.add(f"dep:{self.dep_source}")
        return flags

    # ----- interactive wizard ----------------------------------------------------------

    def _run_wizard(self, args: Dict[str, Any]) -> Dict[str, Any]:
        """Prompt for the inputs (Enter accepts the [default]) and return the filled args.
        Drives the same args dict the non-interactive flow consumes, so generation is
        identical regardless of how the inputs were collected."""
        print("Interactive component scaffolding — press Enter to accept the [default].\n")
        langs = sorted(DEFAULT_TEMPLATE_SOURCES.keys())
        args['language'] = self._prompt_choice("Language", langs, args.get('language'))
        args['name'] = self._prompt(
            "Fully-qualified component name (e.g. com.example.MyComponent)",
            args.get('name'), required=True)
        args['description'] = self._prompt(
            "Description", args.get('description') or "This is a Greengrass v2 component")
        args['platforms'] = self._prompt_multi(
            "Target platform(s)", VALID_PLATFORMS, self._parse_platforms(args.get('platforms')))
        args['dep_source'] = self._prompt_choice(
            "ggcommons dependency source", list(VALID_DEP_SOURCES),
            (args.get('dep_source') or _DEFAULT_DEP_SOURCE))
        if args['dep_source'] == "local" and args['language'] in _GGCOMMONS_PATH_DEFAULTS:
            default_path = self._default_ggcommons_path(args['language'])
            args['ggcommons_path'] = self._prompt(
                "Path to the local ggcommons library", args.get('ggcommons_path') or default_path)
        args['author'] = self._prompt("Author", args.get('author') or "Amazon Web Services")
        print()
        return args

    @staticmethod
    def _prompt(label: str, default: Optional[str] = None, required: bool = False) -> str:
        suffix = f" [{default}]" if default else ""
        while True:
            val = input(f"{label}{suffix}: ").strip()
            if val:
                return val
            if default:
                return default
            if not required:
                return ""
            print("  (a value is required)")

    @classmethod
    def _prompt_choice(cls, label: str, choices, default: Optional[str] = None) -> str:
        """Prompt for one of ``choices`` (case-insensitive); returns the canonical value."""
        while True:
            val = cls._prompt(f"{label} ({'/'.join(choices)})", default, required=not default)
            for c in choices:
                if val.lower() == c.lower():
                    return c
            print(f"  choose one of: {', '.join(choices)}")

    @classmethod
    def _prompt_multi(cls, label: str, choices, default_set) -> str:
        """Prompt for a comma-separated subset of ``choices``; returns a comma string."""
        default_str = ",".join(c for c in choices if c in default_set) or ",".join(choices)
        while True:
            val = cls._prompt(f"{label} — comma-separated from {','.join(choices)}", default_str)
            picked = [p.strip().upper() for p in val.split(",") if p.strip()]
            bad = [p for p in picked if p not in choices]
            if bad:
                print(f"  unknown: {', '.join(bad)}; choose from {', '.join(choices)}")
                continue
            if picked:
                return ",".join(picked)
            print("  pick at least one")

    def _validate_args(self):
        """Validate inputs before touching the filesystem."""
        if not self.component_full_name or self.component_full_name == "ComponentSkeleton":
            raise ValueError("A component name is required (-n/--name, or run with -i/--interactive).")
        if not self.component_language:
            raise ValueError(
                "A language is required (-l/--language, or run with -i/--interactive). "
                f"One of: {', '.join(sorted(DEFAULT_TEMPLATE_SOURCES.keys()))}."
            )
        bad_platforms = self.platforms - set(VALID_PLATFORMS)
        if bad_platforms:
            raise ValueError(
                f"Unknown platform(s): {', '.join(sorted(bad_platforms))}. "
                f"Valid: {', '.join(VALID_PLATFORMS)}."
            )
        if self.dep_source not in VALID_DEP_SOURCES:
            raise ValueError(
                f"Unknown dependency source '{self.dep_source}'. "
                f"Valid: {', '.join(VALID_DEP_SOURCES)}."
            )
        # A local path dependency (Rust/TS) needs a valid library path; a `registry`
        # dependency source resolves from the published artifact instead, so skip the check.
        if self.dep_source == "local" and self.component_language in _GGCOMMONS_PATH_DEFAULTS:
            if not self.ggcommons_path or not os.path.isdir(self.ggcommons_path):
                raise ValueError(
                    f"{self.component_language} components need a valid ggcommons library "
                    f"path, but '{self.ggcommons_path}' does not exist. "
                    f"Pass --ggcommons-path <abs path>."
                )
            # Normalize to forward slashes (Cargo/npm `file:` deps accept them on all OSes).
            self.ggcommons_path = self.ggcommons_path.replace("\\", "/")

    def _guard_target_dir(self, target_dir: str):
        """Refuse to write into a non-empty existing directory unless --force."""
        if os.path.isdir(target_dir) and os.listdir(target_dir):
            if not self.force:
                raise FileExistsError(
                    f"Target directory '{target_dir}' already exists and is not empty. "
                    f"Use --force to overwrite."
                )
            print(f"--force: overwriting existing directory '{target_dir}'.")
            shutil.rmtree(target_dir)

    # ----- template fetching (git URL or local dir) ------------------------------------

    def _fetch_template(self, source: str, target_dir: str):
        """Populate target_dir from a template source: a local directory (copied) or a
        git URL (cloned). Fails fast so substitution never runs on a missing scaffold."""
        if os.path.isdir(source):
            self._copy_tree_into(source, target_dir)
            print(f"Template copied from '{source}'.")
        else:
            self._clone_into(source, target_dir)
            print(f"Template cloned from '{source}'.")

    def _copy_tree_into(self, src: str, target_dir: str):
        items = [i for i in os.listdir(src) if i not in ('.git', '.idea', 'target', '__pycache__')]
        if not items:
            raise RuntimeError(f"Template '{src}' contains no files.")
        os.makedirs(target_dir, exist_ok=True)
        for item in items:
            s = os.path.join(src, item)
            d = os.path.join(target_dir, item)
            if os.path.isdir(s):
                shutil.copytree(s, d, dirs_exist_ok=True)
            else:
                shutil.copy2(s, d)

    def _clone_into(self, repo_url: str, target_dir: str):
        temp_dir = None
        try:
            temp_dir = tempfile.mkdtemp()
            clone_kwargs = {"branch": self.template_ref} if self.template_ref else {}
            try:
                Repo.clone_from(repo_url, temp_dir, **clone_kwargs)
            except GitCommandError as e:
                raise RuntimeError(f"Failed to clone template from '{repo_url}': {e}") from e
            self._copy_tree_into(temp_dir, target_dir)
        finally:
            if temp_dir and os.path.exists(temp_dir):
                try:
                    shutil.rmtree(temp_dir)
                except OSError:
                    pass

    # ----- manifest-driven generation --------------------------------------------------

    def _read_manifest(self, target_dir: str) -> Optional[dict]:
        """Read and remove the template manifest, or return None if absent."""
        mpath = os.path.join(target_dir, MANIFEST_NAME)
        if not os.path.isfile(mpath):
            return None
        with open(mpath, 'r', encoding='utf-8') as fh:
            manifest = json.load(fh)
        os.remove(mpath)  # the manifest is a template artifact; don't ship it
        return manifest

    def _apply_manifest(self, manifest: dict, target_dir: str, values: Dict[str, str]):
        """Apply a template manifest. Schema:
            {
              "required":      ["GGCOMMONS_PATH", ...],       # placeholders that must be non-empty
              "substitutions": {"relative/path": ["TOKEN", ...], ...},
              "renames":       [{"from": "a/{TOKEN}", "to": "b/{TOKEN}"}, ...],
              "conditional":   [{"when": "platform:KUBERNETES", "paths": ["Dockerfile", "k8s"]}, ...]
            }
        ``conditional`` entries are OPTIONAL artifacts: each is kept only when its ``when``
        flag is active (see _condition_flags — one ``platform:<P>`` per selected platform,
        plus ``dep:<source>``); otherwise its paths are removed. substitutions/renames that
        reference a removed path are skipped (not an error), so a template can list a
        conditional file in both ``substitutions`` and ``conditional``.
        """
        for ph in manifest.get("required", []):
            if not values.get(ph):
                raise ValueError(f"Template requires a value for <<{ph}>> but it is empty.")

        # Prune optional artifacts whose condition is not satisfied, recording what was removed
        # so substitutions/renames can skip them gracefully.
        flags = self._condition_flags()
        pruned = []
        for cond in manifest.get("conditional", []):
            if cond.get("when") in flags:
                continue
            for rel in cond.get("paths", []):
                self._remove_path(os.path.join(target_dir, *rel.split("/")))
                pruned.append(rel.rstrip("/"))

        def _is_pruned(relpath: str) -> bool:
            rp = relpath.rstrip("/")
            return any(rp == pr or rp.startswith(pr + "/") for pr in pruned)

        for relpath, placeholders in manifest.get("substitutions", {}).items():
            fpath = os.path.join(target_dir, *relpath.split("/"))
            if not os.path.isfile(fpath):
                if _is_pruned(relpath):
                    continue  # intentionally removed by an unmet "conditional"
                raise RuntimeError(f"Manifest references a file not in the template: '{relpath}'.")
            self.replace_in_file(fpath, {f"<<{ph}>>": values.get(ph, "") for ph in placeholders})

        for rename in manifest.get("renames", []):
            frm_rel = self._interp(rename["from"], values)
            frm = os.path.join(target_dir, *frm_rel.split("/"))
            if not os.path.exists(frm) and _is_pruned(frm_rel):
                continue  # the source was removed by an unmet "conditional"
            to = os.path.join(target_dir, *self._interp(rename["to"], values).split("/"))
            self.rename_file_or_directory(frm, to)

        # Renames/prunes can leave empty parent dirs behind (e.g. the old Java package path).
        self._prune_empty_dirs(target_dir)

    @staticmethod
    def _remove_path(path: str):
        """Remove a file or directory tree if present (used to drop unselected optional artifacts)."""
        if os.path.isdir(path):
            shutil.rmtree(path, ignore_errors=True)
        elif os.path.exists(path):
            try:
                os.remove(path)
            except OSError:
                pass

    @staticmethod
    def _prune_empty_dirs(target_dir: str):
        for root, _dirs, _files in os.walk(target_dir, topdown=False):
            if os.path.abspath(root) == os.path.abspath(target_dir):
                continue
            try:
                if not os.listdir(root):
                    os.rmdir(root)
            except OSError:
                pass

    @staticmethod
    def _interp(template: str, values: Dict[str, str]) -> str:
        """Replace {TOKEN} occurrences in a path with their values."""
        def repl(m):
            key = m.group(1)
            if key not in values:
                raise KeyError(f"Unknown placeholder '{{{key}}}' in manifest rename.")
            return values[key]
        return re.sub(r"\{([A-Z_]+)\}", repl, template)

    # ----- post-generation checks (shared) ---------------------------------------------

    def _verify_no_leftover_tokens(self, target_dir: str):
        """Fail if any '<<TOKEN>>' placeholders remain (template/CLI drift)."""
        leftovers = []
        for root, _dirs, files in os.walk(target_dir):
            if ".git" in root:
                continue
            for fname in files:
                fpath = os.path.join(root, fname)
                try:
                    with open(fpath, 'r', encoding='utf-8') as fh:
                        for lineno, line in enumerate(fh, 1):
                            if "<<" in line and ">>" in line:
                                leftovers.append(f"{fpath}:{lineno}: {line.strip()}")
                except (UnicodeDecodeError, OSError):
                    continue  # skip binary/unreadable files
        if leftovers:
            raise RuntimeError(
                "Unsubstituted '<<...>>' placeholders remain in the generated project "
                "(template/CLI drift):\n  " + "\n  ".join(leftovers)
            )

    def _lint_recipe(self, recipe_path: str):
        """Warn about recipe constructs that break `gdk component publish`."""
        if not os.path.isfile(recipe_path):
            return
        from ggcommons_cli.recipe_lint import lint_recipe_file
        for w in lint_recipe_file(recipe_path):
            print(f"WARNING (recipe): {w}")

    # ----- shared file ops -------------------------------------------------------------

    def rename_file_or_directory(self, old_path: str, new_path: str):
        if not os.path.exists(old_path):
            raise RuntimeError(f"Cannot rename: '{old_path}' does not exist.")
        os.makedirs(os.path.dirname(new_path), exist_ok=True)
        shutil.move(old_path, new_path)

    def replace_in_file(self, file_path: str, replacements: Dict[str, str]):
        with open(file_path, 'r', encoding='utf-8') as file:
            lines = file.readlines()
        for i, line in enumerate(lines):
            for old_str, new_str in replacements.items():
                line = line.replace(old_str, new_str)
            lines[i] = line
        with open(file_path, 'w', encoding='utf-8') as file:
            file.writelines(lines)
