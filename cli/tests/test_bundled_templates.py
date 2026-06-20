"""The component templates must be bundled into the wheel so an installed CLI can
scaffold offline (no repo checkout / network). Builds a wheel and asserts each
language's template — including its manifest — ships inside the package."""
import pathlib
import subprocess
import sys
import zipfile

import pytest

CLI_DIR = pathlib.Path(__file__).resolve().parents[1]  # the cli/ project root


def test_templates_are_bundled_into_the_wheel(tmp_path):
    out = tmp_path / "dist"
    result = subprocess.run(
        [sys.executable, "-m", "pip", "wheel", str(CLI_DIR),
         "--no-deps", "--no-build-isolation", "-w", str(out)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        pytest.skip(f"could not build wheel in this environment:\n{result.stderr[-500:]}")

    wheels = list(out.glob("ggcommons_cli-*.whl"))
    assert wheels, "no wheel produced"
    names = set(zipfile.ZipFile(wheels[0]).namelist())
    for lang in ("java", "python", "rust"):
        manifest = f"ggcommons_cli/templates/{lang}/ggcommons-template.json"
        assert manifest in names, f"{lang} template (manifest) not bundled: {manifest}"
