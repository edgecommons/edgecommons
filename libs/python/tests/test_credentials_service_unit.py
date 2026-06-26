"""Unit tests for the credential service value types, convenience getters, typed
views, and stats — backed by an in-process file vault (no AWS / central sync)."""
import time

import pytest

from ggcommons.credentials.config import open_from_config
from ggcommons.credentials.service import (
    CredentialService,
    DefaultCredentialService,
    Secret,
    CredentialStats,
)
from ggcommons.credentials.errors import CredentialError


@pytest.fixture
def svc(tmp_path):
    return open_from_config({"vault": {"path": str(tmp_path / "vault")}})


class TestSecretValue:
    def test_bytes_and_str_and_json(self):
        s = Secret("n", "1", b'{"a": 1}', {}, 0, "local", "application/json")
        assert s.bytes() == b'{"a": 1}'
        assert s.as_str() == '{"a": 1}'
        assert s.as_json() == {"a": 1}

    def test_repr_redacts_value(self):
        s = Secret("n", "1", b"supersecret", {}, 0, "local", "text/plain")
        r = repr(s)
        assert "supersecret" not in r
        assert "redacted" in r

    def test_as_str_invalid_utf8_raises(self):
        s = Secret("n", "1", b"\xff\xfe\xff", {}, 0, "local", "x")
        with pytest.raises(CredentialError, match="not valid UTF-8"):
            s.as_str()

    def test_as_json_invalid_raises(self):
        s = Secret("n", "1", b"not-json", {}, 0, "local", "x")
        with pytest.raises(CredentialError, match="not JSON"):
            s.as_json()


class TestConvenienceGetters:
    def test_get_bytes_string_json(self, svc):
        svc.put("k", b'{"v": 1}')
        assert svc.get_bytes("k") == b'{"v": 1}'
        assert svc.get_string("k") == '{"v": 1}'
        assert svc.get_json("k") == {"v": 1}

    def test_missing_getters_return_none(self, svc):
        assert svc.get_bytes("absent") is None
        assert svc.get_string("absent") is None
        assert svc.get_json("absent") is None


class TestTypedViews:
    def test_aws_credentials(self, svc):
        svc.put("aws", b'{"accessKeyId": "AKIA", "secretAccessKey": "sk", "sessionToken": "tok"}')
        creds = svc.get_aws_credentials("aws")
        assert creds.access_key_id == "AKIA"
        assert creds.secret_access_key == "sk"
        assert creds.session_token == "tok"

    def test_basic_auth(self, svc):
        svc.put("basic", b'{"username": "u", "password": "p"}')
        ba = svc.get_basic_auth("basic")
        assert ba.username == "u" and ba.password == "p"

    def test_tls_bundle(self, svc):
        svc.put("tls", b'{"certPem": "C", "keyPem": "K", "caPem": "CA"}')
        tls = svc.get_tls_bundle("tls")
        assert tls.cert_pem == "C" and tls.key_pem == "K" and tls.ca_pem == "CA"

    def test_kafka_sasl(self, svc):
        svc.put("kafka", b'{"username": "ku", "password": "kp"}')
        k = svc.get_kafka_sasl("kafka")
        assert k.username == "ku" and k.password == "kp"
        assert k.mechanism == "PLAIN"

    def test_typed_views_missing_return_none(self, svc):
        assert svc.get_aws_credentials("absent") is None
        assert svc.get_basic_auth("absent") is None
        assert svc.get_tls_bundle("absent") is None
        assert svc.get_kafka_sasl("absent") is None


class TestVaultOps:
    def test_put_get_versions_exists_delete(self, svc):
        v1 = svc.put("token", b"a")
        v2 = svc.put("token", b"b")
        assert v1 != v2
        assert svc.exists("token") is True
        assert svc.get("token").as_str() == "b"
        assert svc.get_version("token", v1).as_str() == "a"
        versions = svc.versions("token")
        assert v1 in versions and v2 in versions
        assert svc.delete("token") is True
        assert svc.exists("token") is False
        assert svc.delete("token") is False  # already gone

    def test_get_missing_returns_none(self, svc):
        assert svc.get("nope") is None
        assert svc.get_version("nope", "00000001") is None

    def test_list_returns_metadata(self, svc):
        svc.put("a", b"1")
        svc.put("b", b"2")
        names = {m.name for m in svc.list("")}
        assert {"a", "b"} <= names


class TestStatsAndRefresh:
    def test_stats_no_sync(self, svc):
        svc.put("a", b"1")
        stats = svc.stats()
        assert isinstance(stats, CredentialStats)
        assert stats.secret_count >= 1
        assert stats.last_sync_age_ms is None

    def test_refresh_no_sync_is_noop(self, svc):
        svc.refresh()  # no exception

    def test_refresh_and_stats_with_sync(self, svc):
        class FakeSync:
            def __init__(self):
                self.synced = False

            def sync_now(self):
                self.synced = True

            def stats(self):
                return (int(time.time() * 1000) - 1000, 2, 3)

        fake = FakeSync()
        svc._sync = fake
        svc.refresh()
        assert fake.synced is True
        stats = svc.stats()
        assert stats.sync_failures == 2
        assert stats.rotations == 3
        assert stats.last_sync_age_ms is not None and stats.last_sync_age_ms >= 0


class TestBaseInterface:
    def test_base_refresh_returns_none(self):
        assert CredentialService().refresh() is None

    def test_base_stats_handles_not_implemented(self):
        # base list() raises NotImplementedError -> stats() catches it, count = 0
        stats = CredentialService().stats()
        assert stats.secret_count == 0

    def test_with_audit_is_fluent(self, tmp_path):
        svc = open_from_config({"vault": {"path": str(tmp_path / "v")}})
        assert isinstance(svc, DefaultCredentialService)
        returned = svc.with_audit(None)
        assert returned is svc
