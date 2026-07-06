import json
from types import SimpleNamespace

import pytest

import edgecommons_cli.commands.list_components as lc
from edgecommons_cli.commands.list_components import ListComponents, _filter, _load_catalog

SAMPLE = {
    "schemaVersion": 1,
    "components": [
        {
            "name": "opcua-adapter",
            "repo": "edgecommons/opcua-adapter",
            "language": "JAVA",
            "category": "adapter",
            "description": "OPC UA southbound adapter",
        },
        {
            "name": "modbus-adapter",
            "repo": "edgecommons/modbus-adapter",
            "language": "PYTHON",
            "category": "adapter",
            "description": "Modbus southbound adapter",
        },
        {
            "name": "rollup",
            "repo": "edgecommons/rollup",
            "language": "RUST",
            "category": "processor",
            "description": "Edge aggregation processor",
        },
    ],
}


def _write(tmp_path, data):
    p = tmp_path / "components.json"
    p.write_text(json.dumps(data), encoding="utf-8")
    return str(p)


class TestListComponents:
    def test_lists_all(self, tmp_path, capsys):
        ListComponents().execute_command({"source": _write(tmp_path, SAMPLE)})
        out = capsys.readouterr().out
        assert "opcua-adapter" in out
        assert "modbus-adapter" in out
        assert "rollup" in out
        assert "https://github.com/edgecommons/opcua-adapter" in out

    def test_filter_language_case_insensitive(self, tmp_path, capsys):
        ListComponents().execute_command({"source": _write(tmp_path, SAMPLE), "language": "python"})
        out = capsys.readouterr().out
        assert "modbus-adapter" in out
        assert "opcua-adapter" not in out

    def test_filter_category(self, tmp_path, capsys):
        ListComponents().execute_command(
            {"source": _write(tmp_path, SAMPLE), "category": "processor"}
        )
        out = capsys.readouterr().out
        assert "rollup" in out
        assert "opcua-adapter" not in out

    def test_json_output(self, tmp_path, capsys):
        ListComponents().execute_command({"source": _write(tmp_path, SAMPLE), "json": True})
        parsed = json.loads(capsys.readouterr().out)
        assert len(parsed) == 3

    def test_no_match_is_clean(self, tmp_path, capsys):
        ListComponents().execute_command({"source": _write(tmp_path, SAMPLE), "language": "go"})
        assert "No components matched" in capsys.readouterr().out

    def test_missing_file_raises(self, tmp_path):
        with pytest.raises(RuntimeError):
            _load_catalog(str(tmp_path / "nope.json"))

    def test_malformed_registry_raises(self, tmp_path):
        bad = tmp_path / "bad.json"
        bad.write_text('{"not_components": []}', encoding="utf-8")
        with pytest.raises(RuntimeError):
            _load_catalog(str(bad))

    def test_filter_helper(self):
        assert len(_filter(SAMPLE["components"], "JAVA", None)) == 1
        assert len(_filter(SAMPLE["components"], None, "adapter")) == 2
        assert len(_filter(SAMPLE["components"], None, None)) == 3

    def test_default_reads_private_registry_via_gh(self, monkeypatch, capsys):
        # No --source / env → the gh-authenticated path is used.
        monkeypatch.delenv("EDGECOMMONS_REGISTRY_URL", raising=False)
        monkeypatch.setattr(lc, "_load_text_via_gh", lambda repo, path, ref: json.dumps(SAMPLE))
        ListComponents().execute_command({})
        out = capsys.readouterr().out
        assert "opcua-adapter" in out
        assert "gh:edgecommons/registry" in out

    def test_gh_not_installed_raises(self, monkeypatch):
        def _missing(*a, **k):
            raise FileNotFoundError()

        monkeypatch.setattr(lc.subprocess, "run", _missing)
        with pytest.raises(RuntimeError, match="GitHub CLI"):
            lc._load_text_via_gh("edgecommons/registry", "components.json", "main")

    def test_gh_error_returncode_raises(self, monkeypatch):
        monkeypatch.setattr(
            lc.subprocess, "run",
            lambda *a, **k: SimpleNamespace(returncode=1, stdout="", stderr="HTTP 404: Not Found"),
        )
        with pytest.raises(RuntimeError, match="gh api"):
            lc._load_text_via_gh("edgecommons/registry", "components.json", "main")
