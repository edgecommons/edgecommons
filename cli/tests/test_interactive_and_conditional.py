"""Tests for the interactive wizard + conditional (platform-gated) artifact generation
added to `create-component`. A SYNTHETIC template exercises the conditional mechanism in
isolation (independent of the real language templates / their k8s artifacts)."""
import builtins
import json
import pathlib

import pytest

from ggcommons_cli.commands.create_component import CreateComponent


def _make_template(tmp_path: pathlib.Path) -> pathlib.Path:
    """A minimal template: a base file always generated + a Dockerfile and a k8s/ dir that
    are conditional on `platform:KUBERNETES`. All three carry a token to prove substitution
    runs on the kept files and is skipped (no error) for the pruned ones."""
    t = tmp_path / "tmpl"
    (t / "k8s").mkdir(parents=True)
    (t / "base.txt").write_text("component=<<COMPONENTNAME>>\n", encoding="utf-8")
    (t / "Dockerfile").write_text("# image for <<COMPONENTNAME>>\n", encoding="utf-8")
    (t / "k8s" / "deployment.yaml").write_text("name: <<COMPONENTNAME>>\n", encoding="utf-8")
    manifest = {
        "language": "PYTHON",
        "substitutions": {
            "base.txt": ["COMPONENTNAME"],
            "Dockerfile": ["COMPONENTNAME"],
            "k8s/deployment.yaml": ["COMPONENTNAME"],
        },
        "conditional": [
            {"when": "platform:KUBERNETES", "paths": ["Dockerfile", "k8s"]},
        ],
    }
    (t / "ggcommons-template.json").write_text(json.dumps(manifest), encoding="utf-8")
    return t


def _args(template, out_dir, **overrides):
    args = {
        "name": "com.example.GenTest",
        "description": "A generated test component",
        "language": "PYTHON",
        "path": str(out_dir),
        "jar": None,
        "author": "Tester",
        "bucket": "test-bucket",
        "region": "us-east-1",
        "ggcommons_path": None,
        "template_url": str(template),
        "template_ref": None,
        "force": True,
    }
    args.update(overrides)
    return args


class TestConditionalGeneration:
    def test_k8s_artifacts_pruned_when_kubernetes_not_selected(self, tmp_path):
        template = _make_template(tmp_path)
        CreateComponent().execute_command(
            _args(template, tmp_path, platforms="GREENGRASS,HOST"))
        project = tmp_path / "GenTest"
        assert (project / "base.txt").read_text().strip() == "component=GenTest"
        # Dockerfile + k8s/ are conditional on KUBERNETES (not selected) -> removed.
        assert not (project / "Dockerfile").exists()
        assert not (project / "k8s").exists()
        # Their substitutions were skipped, not errored (no leftover tokens anywhere).
        assert "ggcommons-template.json" not in [p.name for p in project.iterdir()]

    def test_k8s_artifacts_kept_when_kubernetes_selected(self, tmp_path):
        template = _make_template(tmp_path)
        CreateComponent().execute_command(
            _args(template, tmp_path, platforms="HOST,KUBERNETES"))
        project = tmp_path / "GenTest"
        assert (project / "Dockerfile").read_text().strip() == "# image for GenTest"
        assert (project / "k8s" / "deployment.yaml").read_text().strip() == "name: GenTest"

    def test_default_platforms_keep_everything(self, tmp_path):
        """No --platforms (the non-interactive default = all) keeps the conditional artifacts —
        backward-compatible: a template's optional files are emitted unless explicitly excluded."""
        template = _make_template(tmp_path)
        CreateComponent().execute_command(_args(template, tmp_path))  # no platforms key
        project = tmp_path / "GenTest"
        assert (project / "Dockerfile").exists()
        assert (project / "k8s" / "deployment.yaml").exists()

    def test_unknown_platform_rejected(self, tmp_path):
        template = _make_template(tmp_path)
        with pytest.raises(ValueError, match="Unknown platform"):
            CreateComponent().execute_command(_args(template, tmp_path, platforms="MARS"))


class TestDepSource:
    def test_registry_dep_source_skips_ggcommons_path_requirement(self, tmp_path):
        """RUST/TS normally require a valid --ggcommons-path (local path dep). With
        dep-source=registry the component resolves from the published artifact, so the
        local-path check is skipped."""
        template = _make_template(tmp_path)
        # language RUST would require ggcommons_path under 'local'; 'registry' must not.
        CreateComponent().execute_command(
            _args(template, tmp_path, language="RUST", dep_source="registry", ggcommons_path=None))
        assert (tmp_path / "GenTest" / "base.txt").exists()

    def test_local_dep_source_still_requires_path_for_rust(self, tmp_path):
        template = _make_template(tmp_path)
        with pytest.raises(ValueError, match="ggcommons library"):
            CreateComponent().execute_command(
                _args(template, tmp_path, language="RUST", dep_source="local",
                      ggcommons_path=str(tmp_path / "nope")))

    def test_unknown_dep_source_rejected(self, tmp_path):
        template = _make_template(tmp_path)
        with pytest.raises(ValueError, match="dependency source"):
            CreateComponent().execute_command(_args(template, tmp_path, dep_source="ftp"))


class TestParsePlatforms:
    def test_none_defaults_to_all(self):
        assert CreateComponent._parse_platforms(None) == {"GREENGRASS", "HOST", "KUBERNETES"}

    def test_comma_string_uppercased(self):
        assert CreateComponent._parse_platforms("host, kubernetes") == {"HOST", "KUBERNETES"}

    def test_iterable_input(self):
        assert CreateComponent._parse_platforms(["greengrass"]) == {"GREENGRASS"}


class TestDepSourceWiring:
    """The dep-source choice drives the ggcommons dependency declaration in the REAL
    Rust/TS templates via the GGCOMMONS_DEP substitution."""

    _WS = pathlib.Path(__file__).resolve().parents[2]

    def _gen(self, tmp_path, language, tmpl, **overrides):
        args = {
            "name": "com.example.DepGen", "language": language, "path": str(tmp_path),
            "template_url": str(self._WS / "templates" / tmpl), "force": True,
        }
        args.update(overrides)
        CreateComponent().execute_command(args)
        return tmp_path / "DepGen"

    def test_rust_registry_uses_git_dep(self, tmp_path):
        if not (self._WS / "templates" / "rust").is_dir():
            pytest.skip("rust template not present")
        proj = self._gen(tmp_path, "RUST", "rust", dep_source="registry")
        cargo = (proj / "Cargo.toml").read_text()
        dep = [l for l in cargo.splitlines() if l.startswith("ggcommons =")][0]
        assert 'git = "https://github.com/mbreissi/ggcommons"' in dep
        assert "path =" not in dep  # the ggcommons line must not be a path dep

    def test_rust_local_uses_path_dep(self, tmp_path):
        ws = self._WS
        if not (ws / "templates" / "rust").is_dir() or not (ws / "libs" / "rust").is_dir():
            pytest.skip("rust template/lib not present")
        proj = self._gen(tmp_path, "RUST", "rust", dep_source="local",
                         ggcommons_path=str(ws / "libs" / "rust"))
        dep = [l for l in (proj / "Cargo.toml").read_text().splitlines() if l.startswith("ggcommons =")][0]
        assert "path =" in dep and "git =" not in dep

    def test_ts_registry_uses_mbreissi_scope(self, tmp_path):
        if not (self._WS / "templates" / "typescript").is_dir():
            pytest.skip("ts template not present")
        proj = self._gen(tmp_path, "TYPESCRIPT", "typescript", dep_source="registry")
        pkg = (proj / "package.json").read_text()
        assert '"@mbreissi/ggcommons": "^0.1.0"' in pkg
        assert "@breissinger" not in pkg
        # the source imports must use the new scope too
        assert "@breissinger" not in (proj / "src" / "main.ts").read_text()
        # registry dep-source must ship the consumer .npmrc mapping @mbreissi -> GitHub Packages
        npmrc = (proj / ".npmrc").read_text()
        assert "@mbreissi:registry=https://npm.pkg.github.com" in npmrc

    def test_ts_local_omits_npmrc(self, tmp_path):
        if not (self._WS / "templates" / "typescript").is_dir():
            pytest.skip("ts template not present")
        proj = self._gen(tmp_path, "TYPESCRIPT", "typescript", dep_source="local",
                         ggcommons_path=str(self._WS / "libs" / "ts"))
        assert not (proj / ".npmrc").exists()  # local file: dep needs no registry config
        assert 'file:' in (proj / "package.json").read_text()


class TestWizard:
    def test_wizard_fills_inputs_and_gates_artifacts(self, tmp_path, monkeypatch):
        template = _make_template(tmp_path)
        # Answers in prompt order: language, name, description(blank->default),
        # platforms (GREENGRASS,HOST -> no k8s), dep_source, author(blank->default).
        answers = iter([
            "PYTHON",
            "com.example.WizComp",
            "",
            "GREENGRASS,HOST",
            "registry",
            "",
        ])
        monkeypatch.setattr(builtins, "input", lambda _prompt="": next(answers))
        # interactive=True forces the wizard regardless of TTY; name omitted so the wizard supplies it.
        CreateComponent().execute_command(
            _args(template, tmp_path, name=None, language=None, interactive=True))
        project = tmp_path / "WizComp"
        assert project.is_dir()
        assert (project / "base.txt").read_text().strip() == "component=WizComp"
        assert not (project / "k8s").exists()  # wizard picked GREENGRASS,HOST -> k8s pruned

    def test_wizard_kubernetes_selection_keeps_k8s(self, tmp_path, monkeypatch):
        template = _make_template(tmp_path)
        answers = iter(["PYTHON", "com.example.WizK8s", "", "KUBERNETES", "registry", ""])
        monkeypatch.setattr(builtins, "input", lambda _prompt="": next(answers))
        CreateComponent().execute_command(
            _args(template, tmp_path, name=None, language=None, interactive=True))
        assert (tmp_path / "WizK8s" / "k8s" / "deployment.yaml").exists()
