import json

import pytest

from ggcommons_cli.commands.deploy import Deploy


def test_read_name_version(tmp_path):
    (tmp_path / "gdk-config.json").write_text(
        json.dumps({"component": {"com.example.Y": {"version": "1.0.0"}}})
    )
    assert Deploy()._read_name_version(str(tmp_path)) == ("com.example.Y", "1.0.0")


def test_read_name_version_rejects_next_patch(tmp_path):
    (tmp_path / "gdk-config.json").write_text(
        json.dumps({"component": {"com.example.Y": {"version": "NEXT_PATCH"}}})
    )
    with pytest.raises(RuntimeError):
        Deploy()._read_name_version(str(tmp_path))


def test_deploy_requires_existing_path():
    with pytest.raises(FileNotFoundError):
        Deploy().execute_command({"path": "/no/such/dir", "publish": False})
