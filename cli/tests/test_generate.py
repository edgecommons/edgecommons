import pathlib

import pytest

from ggcommons_cli.commands.create_component import CreateComponent
from ggcommons_cli.recipe_lint import lint_recipe_file

# Templates live as siblings of the CLI repo in the ggcommons workspace.
WORKSPACE = pathlib.Path(__file__).resolve().parents[2]
TEMPLATES = {
    "RUST": WORKSPACE / "templates" / "rust",
    "JAVA": WORKSPACE / "templates" / "java",
    "PYTHON": WORKSPACE / "templates" / "python",
}
GG_LIB = WORKSPACE / "libs" / "rust"


def _args(language, out_dir, template):
    return {
        "name": "com.example.GenTest",
        "description": "A generated test component",
        "language": language,
        "path": str(out_dir),
        "jar": None,
        "author": "Tester",
        "bucket": "test-bucket",
        "region": "us-east-1",
        "ggcommons_path": str(GG_LIB),
        "template_url": str(template),
        "template_ref": None,
        "force": True,
    }


def _assert_no_leftover_tokens(project: pathlib.Path):
    for p in project.rglob("*"):
        if not p.is_file():
            continue
        try:
            text = p.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        assert not ("<<" in text and ">>" in text), f"leftover token in {p}"


@pytest.mark.parametrize("language", ["RUST", "JAVA", "PYTHON"])
def test_generate_component(language, tmp_path):
    template = TEMPLATES[language]
    if not template.is_dir():
        pytest.skip(f"template not present: {template}")
    if language == "RUST" and not GG_LIB.is_dir():
        pytest.skip("ggcommons-rust-lib not present")

    # execute_command raises on leftover tokens / clone failure (the #1 guards).
    CreateComponent().execute_command(_args(language, tmp_path, template))

    project = tmp_path / "GenTest"
    assert project.is_dir()
    _assert_no_leftover_tokens(project)
    # The template manifest must be stripped from the generated project.
    assert not (project / "ggcommons-template.json").exists()
    # The recipe must be GDK-publish clean.
    assert lint_recipe_file(str(project / "recipe.yaml")) == []


def test_target_dir_guard_without_force(tmp_path):
    template = TEMPLATES["PYTHON"]
    if not template.is_dir():
        pytest.skip("python template not present")
    args = _args("PYTHON", tmp_path, template)
    args["force"] = False
    CreateComponent().execute_command(args)            # first time: ok
    with pytest.raises(FileExistsError):
        CreateComponent().execute_command(args)        # second time: guarded


def test_rust_requires_valid_ggcommons_path(tmp_path):
    template = TEMPLATES["RUST"]
    if not template.is_dir():
        pytest.skip("rust template not present")
    args = _args("RUST", tmp_path, template)
    args["ggcommons_path"] = str(tmp_path / "does-not-exist")
    with pytest.raises(ValueError):
        CreateComponent().execute_command(args)
