"""Packaging shim: metadata lives in pyproject.toml (PEP 621); this only adds a
custom build step that bundles the repo-root templates/ into the wheel so an
installed CLI can scaffold offline (no repo checkout / network needed)."""
import os
import shutil

from setuptools import setup
from setuptools.command.build_py import build_py

_CLI_DIR = os.path.dirname(os.path.abspath(__file__))
# In the monorepo the templates live one level up, at <root>/templates.
_TEMPLATES_SRC = os.path.join(os.path.dirname(_CLI_DIR), "templates")
_LANGS = ("java", "python", "rust", "typescript")
_IGNORE = shutil.ignore_patterns(".git", ".idea", "__pycache__", "target", "out", "*.pyc")


class BuildPyWithTemplates(build_py):
    """Copy templates/<lang> into the built package as ggcommons_cli/templates/<lang>."""

    def run(self):
        super().run()
        if not os.path.isdir(_TEMPLATES_SRC):
            return  # building outside the monorepo: nothing to bundle
        for lang in _LANGS:
            src = os.path.join(_TEMPLATES_SRC, lang)
            if os.path.isdir(src):
                dst = os.path.join(self.build_lib, "ggcommons_cli", "templates", lang)
                shutil.copytree(src, dst, dirs_exist_ok=True, ignore=_IGNORE)


setup(cmdclass={"build_py": BuildPyWithTemplates})
