import json
import os
import re
import shutil
import tempfile

from git import Repo, GitCommandError

from ggcommons_cli import CommandBase
from typing import Dict, Any, Optional

# A template may ship this manifest to drive generation generically (no CLI code
# changes needed to add a language). See _apply_manifest for the schema.
MANIFEST_NAME = "ggcommons-template.json"

# Default template sources per language. Override any with --template-url. A source
# may be a git URL (cloned) or a local directory (copied) — see _fetch_template.
DEFAULT_TEMPLATE_SOURCES = {
    "JAVA": "git@ssh.gitlab.aws.dev:greengrass-commons/component-templates/java-component-template.git",
    "PYTHON": "git@ssh.gitlab.aws.dev:greengrass-commons/component-templates/python-component-template.git",
    "RUST": "git@ssh.gitlab.aws.dev:greengrass-commons/component-templates/rust-component-template.git",
}


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
        self.force = False

    @classmethod
    def get_json_configuration(cls):
        # Default ggcommons (Rust) library path: the sibling ggcommons-rust-lib in the
        # workspace (../../../ from this file: commands -> ggcommons_cli -> ggcommons-cli
        # -> workspace), as an absolute, forward-slash path (TOML/Cargo-friendly).
        default_ggcommons_path = os.path.abspath(
            os.path.join(os.path.dirname(__file__), "..", "..", "..", "ggcommons-rust-lib")
        ).replace("\\", "/")
        return {
            "name": "create-component",
            "description": "Create a new component that uses ggcommons",
            "parameters": [
                {
                    "name": "name",
                    "short": "n",
                    "description": "Fully qualified name of the component",
                    "type": "string",
                    "required": True
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
                    "description": "Programming language",
                    "type": "string",
                    "required": True,
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
                    "description": "Absolute path to the ggcommons Rust library "
                                   "(RUST only; becomes the Cargo path dependency)",
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
                    "name": "force",
                    "short": "f",
                    "description": "Overwrite the target directory if it already exists",
                    "type": "boolean",
                    "required": False
                }
            ]
        }

    def execute_command(self, args: Dict[str, Any]):
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
        self.template_url = args.get('template_url')
        self.force = bool(args.get('force'))

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
        values = self._placeholder_values()
        if manifest is not None:
            # Manifest-driven (generic) — no per-language CLI code required.
            self._apply_manifest(manifest, target_dir, values)
        elif self.component_language == "PYTHON":
            self._legacy_python(target_dir)
        elif self.component_language == "JAVA":
            self._legacy_java(target_dir)
        else:
            raise RuntimeError(
                f"Template for '{self.component_language}' has no '{MANIFEST_NAME}' and no "
                f"built-in generator. Add a manifest to the template."
            )

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
        }

    def _validate_args(self):
        """Validate inputs before touching the filesystem."""
        if not self.component_full_name or self.component_full_name == "ComponentSkeleton":
            raise ValueError("A component name is required (-n/--name).")
        if self.component_language == "RUST" and (
            not self.ggcommons_path or not os.path.isdir(self.ggcommons_path)
        ):
            raise ValueError(
                f"RUST components need a valid ggcommons library path, but "
                f"'{self.ggcommons_path}' does not exist. Pass --ggcommons-path <abs path>."
            )

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
            try:
                Repo.clone_from(repo_url, temp_dir)
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
              "renames":       [{"from": "a/{TOKEN}", "to": "b/{TOKEN}"}, ...]
            }
        """
        for ph in manifest.get("required", []):
            if not values.get(ph):
                raise ValueError(f"Template requires a value for <<{ph}>> but it is empty.")

        for relpath, placeholders in manifest.get("substitutions", {}).items():
            fpath = os.path.join(target_dir, *relpath.split("/"))
            if not os.path.isfile(fpath):
                raise RuntimeError(f"Manifest references a file not in the template: '{relpath}'.")
            self.replace_in_file(fpath, {f"<<{ph}>>": values.get(ph, "") for ph in placeholders})

        for rename in manifest.get("renames", []):
            frm = os.path.join(target_dir, *self._interp(rename["from"], values).split("/"))
            to = os.path.join(target_dir, *self._interp(rename["to"], values).split("/"))
            self.rename_file_or_directory(frm, to)

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

    # ----- legacy per-language generators (used when a template has no manifest) --------

    def _legacy_python(self, base: str):
        self.replace_in_file(os.path.join(base, "recipe.yaml"),
                             {"<<DESCRIPTION>>": self.component_description,
                              "<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<COMPONENTNAME>>": self.component_name,
                              "<<MAINCLASSNAME>>": f"{self.package_name}.{self.component_name}"})
        self.replace_in_file(os.path.join(base, "gdk-config.json"),
                             {"<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<AUTHOR>>": self.author, "<<BUCKET>>": self.bucket,
                              "<<REGION>>": self.region})
        self.replace_in_file(os.path.join(base, "main.py"),
                             {"<<PACKAGE>>": self.package_name,
                              "<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<COMPONENTNAME>>": self.component_name})
        self.replace_in_file(os.path.join(base, "app", "greengrass_app.py"),
                             {"<<PACKAGE>>": self.package_name,
                              "<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<COMPONENTNAME>>": self.component_name})
        self.rename_file_or_directory(os.path.join(base, "app", "greengrass_app.py"),
                                      os.path.join(base, "app", self.component_name + ".py"))

    def _legacy_java(self, base: str):
        old_pkg_dir = os.path.join(base, "src/main/java/com/aws/proserve/testcomponent")
        self.replace_in_file(os.path.join(old_pkg_dir, "App.java"),
                             {"com.aws.proserve.testcomponent": self.component_full_name})
        self.rename_file_or_directory(old_pkg_dir,
                                      os.path.join(base, "src/main/java/com/aws/proserve/" + self.component_name.lower()))
        self.rename_file_or_directory(os.path.join(base, "test-configs/TestComponent.json"),
                                      os.path.join(base, "test-configs", self.component_name + ".json"))
        self.replace_in_file(os.path.join(base, "recipe.yaml"),
                             {"<<DESCRIPTION>>": self.component_description,
                              "<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<JARNAME>>": self.jar_name,
                              "<<MAINCLASSNAME>>": f"{self.package_name}.{self.component_name}"})
        self.replace_in_file(os.path.join(base, "pom.xml"),
                             {"<<PACKAGE>>": self.package_name, "<<JARNAME>>": self.jar_name,
                              "<<COMPONENTNAME>>": self.component_name})
        self.replace_in_file(os.path.join(base, "gdk-config.json"),
                             {"<<COMPONENTFULLNAME>>": self.component_full_name,
                              "<<AUTHOR>>": self.author, "<<BUCKET>>": self.bucket,
                              "<<REGION>>": self.region})
        new_main = os.path.join(base, "src/main/java", self.package_name.replace(".", "/"),
                                self.component_name + ".java")
        self.rename_file_or_directory(os.path.join(base, "src/main/java",
                                                   self.package_name.replace(".", "/"), "App.java"),
                                      new_main)
        self.replace_in_file(new_main,
                             {"<<PACKAGE>>": self.package_name,
                              "<<COMPONENTNAME>>": self.component_name,
                              "<<COMPONENTFULLNAME>>": self.component_full_name})
