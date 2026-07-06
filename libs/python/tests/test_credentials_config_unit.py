"""Unit tests for credentials/config.py (build_key_provider, open_from_config) and the
file/env paths of credentials/keyprovider.py.

Covers the in-process file vault and env-KEK provider; the KMS/PKCS#11 providers are
infra-only and only their config-validation error branches are exercised here.
"""
import base64
import os

import pytest

from edgecommons.credentials.config import build_key_provider, open_from_config, _sync_entries
from edgecommons.credentials.keyprovider import FileKeyProvider, EnvKeyProvider
from edgecommons.credentials.errors import CredentialError
from edgecommons.credentials import crypto


def _b64_key():
    return base64.b64encode(b"k" * crypto.KEY_LEN).decode("ascii")


class TestBuildKeyProviderFile:
    def test_generates_keyfile_when_absent(self, tmp_path):
        key_path = str(tmp_path / "vault.key")
        kp = build_key_provider({"type": "file", "keyPath": key_path}, key_path)
        assert isinstance(kp, FileKeyProvider)
        assert os.path.exists(key_path)

    def test_loads_existing_keyfile(self, tmp_path):
        key_path = str(tmp_path / "vault.key")
        kp1 = build_key_provider({"type": "file", "keyPath": key_path}, key_path)
        kp2 = build_key_provider({"type": "file", "keyPath": key_path}, key_path)
        # Same KEK -> same provider_id and a round-trip wrap/unwrap works across instances.
        wrapped = kp1.wrap_dek("vid", b"d" * crypto.KEY_LEN)
        assert kp2.unwrap_dek("vid", wrapped) == b"d" * crypto.KEY_LEN

    def test_default_type_is_file(self, tmp_path):
        key_path = str(tmp_path / "default.key")
        kp = build_key_provider({}, key_path)
        assert isinstance(kp, FileKeyProvider)
        assert os.path.exists(key_path)


class TestBuildKeyProviderEnv:
    def test_env_provider(self, monkeypatch):
        monkeypatch.setenv("EDGECOMMONS_VAULT_KEK", _b64_key())
        kp = build_key_provider({"type": "env"}, "unused.key")
        assert isinstance(kp, EnvKeyProvider)
        assert kp.provider_id == "env"

    def test_env_provider_custom_var(self, monkeypatch):
        monkeypatch.setenv("MY_KEK", _b64_key())
        kp = build_key_provider({"type": "env", "envVar": "MY_KEK"}, "unused.key")
        assert isinstance(kp, EnvKeyProvider)

    def test_default_type_env_via_profile(self, monkeypatch):
        monkeypatch.setenv("EDGECOMMONS_VAULT_KEK", _b64_key())
        kp = build_key_provider({}, "unused.key", default_type="env")
        assert isinstance(kp, EnvKeyProvider)


class TestBuildKeyProviderErrors:
    def test_kms_requires_key_id(self):
        with pytest.raises(CredentialError, match="kmsKeyId"):
            build_key_provider({"type": "kms"}, "k")

    def test_pkcs11_requires_module_path(self):
        with pytest.raises(CredentialError, match="modulePath"):
            build_key_provider({"type": "pkcs11"}, "k")

    def test_pkcs11_requires_key_label(self):
        with pytest.raises(CredentialError, match="keyLabel"):
            build_key_provider({"type": "pkcs11", "modulePath": "/lib/x.so"}, "k")

    def test_pkcs11_requires_pin(self):
        with pytest.raises(CredentialError, match="pinEnv or keyProvider.pin"):
            build_key_provider(
                {"type": "pkcs11", "modulePath": "/lib/x.so", "keyLabel": "lbl"}, "k"
            )

    def test_pkcs11_pin_env_unset(self):
        with pytest.raises(CredentialError, match="pinEnv 'NOPE_PIN' is not set"):
            build_key_provider(
                {"type": "pkcs11", "modulePath": "/lib/x.so", "keyLabel": "lbl", "pinEnv": "NOPE_PIN"},
                "k",
            )

    def test_unsupported_type(self):
        with pytest.raises(CredentialError, match="is not supported"):
            build_key_provider({"type": "bogus"}, "k")


class TestEnvKeyProviderErrors:
    def test_unset_env_raises(self, monkeypatch):
        monkeypatch.delenv("ABSENT_KEK", raising=False)
        with pytest.raises(CredentialError, match="unset or empty"):
            EnvKeyProvider("ABSENT_KEK")

    def test_invalid_base64_raises(self, monkeypatch):
        monkeypatch.setenv("BAD_KEK", "!!!not-base64!!!")
        with pytest.raises(CredentialError, match="not valid base64"):
            EnvKeyProvider("BAD_KEK")

    def test_wrong_length_raises(self, monkeypatch):
        monkeypatch.setenv("SHORT_KEK", base64.b64encode(b"short").decode("ascii"))
        with pytest.raises(CredentialError, match="must be 32 bytes"):
            EnvKeyProvider("SHORT_KEK")

    def test_strips_whitespace(self, monkeypatch):
        monkeypatch.setenv("WS_KEK", "  " + _b64_key() + "\n")
        kp = EnvKeyProvider("WS_KEK")
        assert kp.provider_id == "env"

    def test_env_and_file_interop(self, monkeypatch, tmp_path):
        # A vault wrapped by EnvKeyProvider opens with FileKeyProvider holding the same KEK.
        raw = b"z" * crypto.KEY_LEN
        monkeypatch.setenv("INTEROP_KEK", base64.b64encode(raw).decode("ascii"))
        env_kp = EnvKeyProvider("INTEROP_KEK")
        file_kp = FileKeyProvider(raw)
        wrapped = env_kp.wrap_dek("vid", b"d" * crypto.KEY_LEN)
        assert file_kp.unwrap_dek("vid", wrapped) == b"d" * crypto.KEY_LEN


class TestFileKeyProvider:
    def test_wrong_kek_length_raises(self):
        with pytest.raises(CredentialError, match="32 bytes"):
            FileKeyProvider(b"too-short")

    def test_unwrap_missing_nonce_raises(self):
        kp = FileKeyProvider(b"k" * crypto.KEY_LEN)
        with pytest.raises(CredentialError, match="missing wrapNonce"):
            kp.unwrap_dek("vid", {"wrappedDek": "x"})


class TestSyncEntries:
    def test_normalizes_str_and_dict_entries(self):
        cfg = {"secrets": ["plain", {"name": "n", "from": "central-id"}, {"no_name": 1}, 5]}
        assert _sync_entries(cfg) == [("plain", None), ("n", "central-id")]

    def test_empty(self):
        assert _sync_entries(None) == []
        assert _sync_entries({}) == []


class TestOpenFromConfig:
    def test_file_vault_no_central(self, tmp_path):
        vault_path = str(tmp_path / "vault")
        svc = open_from_config({"vault": {"path": vault_path}})
        assert svc is not None
        # round-trips a secret through the opened vault
        svc.put("greeting", b"hello")
        assert svc.get("greeting").as_str() == "hello"

    def test_audit_disabled(self, tmp_path):
        vault_path = str(tmp_path / "vault2")
        svc = open_from_config({"vault": {"path": vault_path}, "audit": {"enabled": False}})
        assert svc is not None

    def test_unsupported_central_raises(self, tmp_path):
        vault_path = str(tmp_path / "vault3")
        with pytest.raises(CredentialError, match="central source 'bogus' is not supported"):
            open_from_config({"vault": {"path": vault_path}, "central": {"type": "bogus"}})

    def test_namespace_applied(self, tmp_path):
        vault_path = str(tmp_path / "vault4")
        svc = open_from_config({"vault": {"path": vault_path}}, namespace="thing/comp")
        svc.put("k", b"v")
        assert svc.get("k").as_str() == "v"
