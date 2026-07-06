"""Extra coverage for ``edgecommons.credentials.config`` success/build branches.

The existing ``test_credentials_config_unit.py`` covers the file/env providers and the
config-validation *error* branches. The branches below are the *successful* construction paths that
require AWS/HSM-backed classes at the call site — they are exercised here by substituting in-process
fakes (no boto3, no PKCS#11 module, no network):

- ``build_key_provider`` returning a ``KmsKeyProvider`` (config.py line 52, ``kms``/``greengrass``).
- ``build_key_provider`` returning a ``Pkcs11KeyProvider`` with a literal ``pin`` (lines 66 + 69)
  and with ``pinEnv`` (line 69 via the env path).
- ``open_from_config`` wiring the ``awsSecretsManager`` central source + sync engine (lines 115-125).
"""
import base64

import pytest

import edgecommons.credentials.config as config_mod
import edgecommons.credentials.central as central_mod
import edgecommons.credentials.sync as sync_mod
from edgecommons.credentials.config import build_key_provider, open_from_config
from edgecommons.credentials import crypto


class TestKmsProviderBuild:
    """config.py line 52: a valid ``kms`` keyProvider builds the KMS provider with passthrough args."""

    def test_kms_provider_built_with_key_id(self, monkeypatch):
        captured = {}

        class FakeKms:
            def __init__(self, key_id, region=None, endpoint_url=None):
                captured["args"] = (key_id, region, endpoint_url)

        monkeypatch.setattr(config_mod, "KmsKeyProvider", FakeKms)
        result = build_key_provider(
            {
                "type": "kms",
                "kmsKeyId": "alias/test",
                "region": "us-east-1",
                "endpointUrl": "http://localhost:4566",
            },
            "unused.key",
        )
        assert isinstance(result, FakeKms)
        assert captured["args"] == ("alias/test", "us-east-1", "http://localhost:4566")

    def test_greengrass_alias_built_with_key_id(self, monkeypatch):
        # The ``greengrass`` alias takes the same kms branch (config.py line 48/52).
        captured = {}

        class FakeKms:
            def __init__(self, key_id, region=None, endpoint_url=None):
                captured["key_id"] = key_id

        monkeypatch.setattr(config_mod, "KmsKeyProvider", FakeKms)
        result = build_key_provider({"type": "greengrass", "kmsKeyId": "k-1"}, "unused.key")
        assert isinstance(result, FakeKms)
        assert captured["key_id"] == "k-1"


class TestPkcs11ProviderBuild:
    """config.py lines 66 + 69: a ``pkcs11`` keyProvider builds the provider with the resolved pin."""

    def test_pkcs11_provider_built_with_literal_pin(self, monkeypatch):
        captured = {}

        class FakePkcs11:
            def __init__(self, module_path, token_label, key_label, pin):
                captured["args"] = (module_path, token_label, key_label, pin)

        monkeypatch.setattr(config_mod, "Pkcs11KeyProvider", FakePkcs11)
        result = build_key_provider(
            {
                "type": "pkcs11",
                "modulePath": "/lib/softhsm2.so",
                "tokenLabel": "tok",
                "keyLabel": "lbl",
                "pin": "1234",  # exercises the literal-pin branch (line 66)
            },
            "unused.key",
        )
        assert isinstance(result, FakePkcs11)
        assert captured["args"] == ("/lib/softhsm2.so", "tok", "lbl", "1234")

    def test_pkcs11_provider_built_with_pin_env(self, monkeypatch):
        # The pinEnv path resolves the pin from the environment and still reaches the return (line 69).
        captured = {}

        class FakePkcs11:
            def __init__(self, module_path, token_label, key_label, pin):
                captured["pin"] = pin

        monkeypatch.setenv("HSM_PIN_VAR", "secret-pin")
        monkeypatch.setattr(config_mod, "Pkcs11KeyProvider", FakePkcs11)
        result = build_key_provider(
            {
                "type": "pkcs11",
                "modulePath": "/lib/softhsm2.so",
                "keyLabel": "lbl",
                "pinEnv": "HSM_PIN_VAR",
            },
            "unused.key",
        )
        assert isinstance(result, FakePkcs11)
        assert captured["pin"] == "secret-pin"


class TestCentralAwsSecretsManager:
    """config.py lines 115-125: the ``awsSecretsManager`` central source + sync engine are wired."""

    def test_central_source_and_sync_engine_wired(self, tmp_path, monkeypatch):
        captured = {}

        class FakeSource:
            def __init__(self, region=None, endpoint_url=None):
                captured["source"] = (region, endpoint_url)

        class FakeEngine:
            def __init__(self, vault, lock, source, namespace, secrets, interval_secs, bootstrap):
                captured["engine"] = (namespace, secrets, interval_secs, bootstrap)

        # Patched on their defining modules because open_from_config imports them lazily
        # (``from .central import ...`` / ``from .sync import ...``) at call time.
        monkeypatch.setattr(central_mod, "AwsSecretsManagerSource", FakeSource)
        monkeypatch.setattr(sync_mod, "SyncEngine", FakeEngine)

        vault_path = str(tmp_path / "vault_central")
        svc = open_from_config(
            {
                "vault": {"path": vault_path},
                "central": {
                    "type": "awsSecretsManager",
                    "region": "us-west-2",
                    "endpointUrl": "http://localhost:4566",
                    "refreshIntervalSecs": 60,
                    "bootstrapOnStart": False,
                    "sync": {"secrets": ["s1", {"name": "s2", "from": "central/s2"}]},
                },
            },
            namespace="thing/comp",
        )

        assert svc is not None
        assert captured["source"] == ("us-west-2", "http://localhost:4566")
        ns, secrets, interval_secs, bootstrap = captured["engine"]
        assert ns == "thing/comp"
        assert secrets == [("s1", None), ("s2", "central/s2")]
        assert interval_secs == 60
        assert bootstrap is False

    def test_central_defaults_applied(self, tmp_path, monkeypatch):
        # No region/endpoint/interval/bootstrap/sync -> the documented defaults flow through.
        captured = {}

        class FakeSource:
            def __init__(self, region=None, endpoint_url=None):
                captured["source"] = (region, endpoint_url)

        class FakeEngine:
            def __init__(self, vault, lock, source, namespace, secrets, interval_secs, bootstrap):
                captured["engine"] = (secrets, interval_secs, bootstrap)

        monkeypatch.setattr(central_mod, "AwsSecretsManagerSource", FakeSource)
        monkeypatch.setattr(sync_mod, "SyncEngine", FakeEngine)

        vault_path = str(tmp_path / "vault_central2")
        svc = open_from_config(
            {"vault": {"path": vault_path}, "central": {"type": "awsSecretsManager"}}
        )

        assert svc is not None
        assert captured["source"] == (None, None)
        secrets, interval_secs, bootstrap = captured["engine"]
        assert secrets == []
        assert interval_secs == 300  # default refreshIntervalSecs
        assert bootstrap is True  # default bootstrapOnStart


def test_crypto_key_len_sanity():
    # Guards the fakes above: the file key provider used by LocalVault.open still needs a real KEK
    # length, so make sure the constant the rest of the suite relies on is intact.
    assert crypto.KEY_LEN == 32
    assert len(base64.b64encode(b"k" * crypto.KEY_LEN)) > 0
