from ggcommons_cli.commands.upgrade import Upgrade


def test_bump_pom(tmp_path):
    pom = tmp_path / "pom.xml"
    pom.write_text(
        "<dependency><groupId>com.aws.proserve</groupId>"
        "<artifactId>ggcommons</artifactId><version>1.1.9-SNAPSHOT</version></dependency>"
    )
    msgs = Upgrade._bump_pom(str(pom), "2.0.0")
    assert "<version>2.0.0</version>" in pom.read_text()
    assert msgs and "2.0.0" in msgs[0]


def test_bump_requirements(tmp_path):
    req = tmp_path / "requirements.txt"
    req.write_text("psutil>=5.9\ngreengrass-commons\nwatchdog>=3\n")
    Upgrade._bump_requirements(str(req), "2.0.0")
    assert "greengrass-commons==2.0.0" in req.read_text()


def test_bump_cargo_version_dep(tmp_path):
    cargo = tmp_path / "Cargo.toml"
    cargo.write_text('[dependencies]\nggcommons = "0.1"\n')
    Upgrade._bump_cargo(str(cargo), "0.2")
    assert 'ggcommons = "0.2"' in cargo.read_text()


def test_bump_cargo_path_dep_is_noop(tmp_path):
    cargo = tmp_path / "Cargo.toml"
    original = '[dependencies]\nggcommons = { path = "../ggcommons-rust-lib" }\n'
    cargo.write_text(original)
    msgs = Upgrade._bump_cargo(str(cargo), "0.2")
    assert cargo.read_text() == original
    assert any("path dependency" in m for m in msgs)


def test_execute_with_no_dependency(tmp_path, capsys):
    Upgrade().execute_command({"path": str(tmp_path), "to": "1.0.0"})
    assert "No ggcommons dependency" in capsys.readouterr().out
