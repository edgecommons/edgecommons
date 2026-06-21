"""Python vault tests: functional behavior + cross-language conformance vs vault-test-vectors/."""
import base64
import json
import os
from pathlib import Path

import pytest

from ggcommons.credentials import (
    CredentialError,
    DefaultCredentialService,
    FileKeyProvider,
    LocalVault,
    open_from_config,
)
from ggcommons.credentials import crypto, format as fmt

VECTORS_DIR = Path(__file__).resolve().parents[3] / "vault-test-vectors"


def _svc(tmp_path) -> DefaultCredentialService:
    provider = FileKeyProvider(bytes([7] * 32))
    return DefaultCredentialService(LocalVault.open(str(tmp_path / "vault"), provider, 2))


def test_put_get_roundtrip_and_typed_views(tmp_path):
    c = _svc(tmp_path)
    c.put("db/password", b"s3cr3t")
    c.put("svc/config", b'{"k":1}')
    assert c.get_string("db/password") == "s3cr3t"
    assert c.get_json("svc/config")["k"] == 1
    assert c.exists("db/password")
    assert c.get("missing") is None
    assert [m.name for m in c.list("")] == ["db/password", "svc/config"]


def test_versions_monotonic_and_pruned(tmp_path):
    c = _svc(tmp_path)  # keep_versions = 2
    c.put("k", b"v1")
    c.put("k", b"v2")
    c.put("k", b"v3")
    assert c.versions("k") == ["00000002", "00000003"]
    assert c.get("k").as_str() == "v3"
    assert c.get_version("k", "00000002").as_str() == "v2"
    assert c.get_version("k", "00000001") is None


def test_persists_and_reopens(tmp_path):
    _svc(tmp_path).put("token", b"abc")
    assert _svc(tmp_path).get_string("token") == "abc"


def test_wrong_kek_fails_closed(tmp_path):
    _svc(tmp_path).put("token", b"abc")
    with pytest.raises(CredentialError):
        LocalVault.open(str(tmp_path / "vault"), FileKeyProvider(bytes([9] * 32)), 2)


def test_tamper_detected(tmp_path):
    path = tmp_path / "vault"
    _svc(tmp_path).put("k", b"v1")
    vf = json.loads(path.read_text())
    ct = base64.b64decode(vf["secrets"]["k"]["versions"][0]["ciphertext"])
    ct = bytes([ct[0] ^ 1]) + ct[1:]
    vf["secrets"]["k"]["versions"][0]["ciphertext"] = base64.b64encode(ct).decode()
    path.write_text(json.dumps(vf))
    with pytest.raises(CredentialError):
        LocalVault.open(str(path), FileKeyProvider(bytes([7] * 32)), 2)


def test_typed_views(tmp_path):
    c = _svc(tmp_path)
    c.put("aws", b'{"accessKeyId":"AKIA","secretAccessKey":"sk","sessionToken":"tok"}')
    c.put("basic", b'{"username":"u","password":"p"}')
    c.put("tls", b'{"certPem":"C","keyPem":"K"}')
    c.put("kafka", b'{"username":"ku","password":"kp"}')
    assert c.get_aws_credentials("aws").access_key_id == "AKIA"
    assert c.get_aws_credentials("aws").session_token == "tok"
    assert c.get_basic_auth("basic").username == "u"
    assert c.get_tls_bundle("tls").cert_pem == "C"
    assert c.get_kafka_sasl("kafka").mechanism == "PLAIN"
    with pytest.raises(CredentialError):
        c.get_aws_credentials("basic")  # wrong shape
    assert c.get_basic_auth("missing") is None


def test_namespacing_isolates_components(tmp_path):
    from ggcommons.credentials import DefaultCredentialService

    path = str(tmp_path / "vault")
    kek = bytes([5] * 32)
    c1 = DefaultCredentialService(LocalVault.open(path, FileKeyProvider(kek), 2), namespace="thing-1/CompA")
    c2 = DefaultCredentialService(LocalVault.open(path, FileKeyProvider(kek), 2), namespace="thing-1/CompB")
    c1.put("db/password", b"a-secret")
    c2.put("db/password", b"b-secret")
    # Same caller-facing key, no collision in the shared vault.
    assert c1.get_string("db/password") == "a-secret"
    assert c2.get_string("db/password") == "b-secret"
    assert [m.name for m in c1.list("")] == ["db/password"]
    raw = (tmp_path / "vault").read_text()
    assert "thing-1/CompA/db/password" in raw
    assert "thing-1/CompB/db/password" in raw


@pytest.mark.skipif(os.environ.get("GGCOMMONS_IT_SM") != "1", reason="needs floci secretsmanager (GGCOMMONS_IT_SM=1)")
def test_central_sync_from_secrets_manager(tmp_path):
    import uuid

    import boto3

    os.environ.setdefault("AWS_ACCESS_KEY_ID", "test")
    os.environ.setdefault("AWS_SECRET_ACCESS_KEY", "test")
    os.environ.setdefault("AWS_REGION", "us-east-1")
    sm = boto3.client("secretsmanager", region_name="us-east-1", endpoint_url="http://localhost:4566")
    name = f"ggcommons-py-cred-{uuid.uuid4()}"
    sm.create_secret(Name=name, SecretString="v1")
    try:
        cfg = {
            "vault": {"path": str(tmp_path / "vault"), "keyProvider": {"type": "file", "keyPath": str(tmp_path / "vault.key")}},
            "central": {
                "type": "awsSecretsManager", "region": "us-east-1", "endpointUrl": "http://localhost:4566",
                "bootstrapOnStart": True, "refreshIntervalSecs": 0, "sync": {"secrets": [name]},
            },
        }
        creds = open_from_config(cfg)  # namespace "" → central id == local key == name
        assert creds.get_string(name) == "v1"

        sm.put_secret_value(SecretId=name, SecretString="v2")
        creds.refresh()
        assert creds.get_string(name) == "v2"
        assert len(creds.versions(name)) >= 2  # previous version retained (rotation grace)

        before = len(creds.versions(name))
        creds.refresh()
        assert len(creds.versions(name)) == before  # no churn when unchanged
    finally:
        sm.delete_secret(SecretId=name, ForceDeleteWithoutRecovery=True)


def test_resolve_secret_refs(tmp_path):
    from ggcommons.credentials import resolve_secret_refs

    c = _svc(tmp_path)
    c.put("kinesis/name", b"my-stream")
    c.put("aws/creds", b'{"accessKeyId":"AKIA","secretAccessKey":"sk"}')

    cfg = {
        "streams": [
            {"name": {"$secret": "kinesis/name"}, "region": "us-east-1"},
        ],
        "auth": {"key": {"$secret": "aws/creds", "field": "accessKeyId"}},
        "plain": "unchanged",
    }
    resolve_secret_refs(cfg, c)
    assert cfg["streams"][0]["name"] == "my-stream"
    assert cfg["streams"][0]["region"] == "us-east-1"
    assert cfg["auth"]["key"] == "AKIA"
    assert cfg["plain"] == "unchanged"

    # Missing secret -> error (fail-closed).
    with pytest.raises(CredentialError):
        resolve_secret_refs({"x": {"$secret": "does/not/exist"}}, c)
    # Missing field -> error.
    with pytest.raises(CredentialError):
        resolve_secret_refs({"x": {"$secret": "aws/creds", "field": "nope"}}, c)


def test_stats_and_credential_stats(tmp_path):
    from ggcommons.credentials import CredentialStats

    c = _svc(tmp_path)
    s = c.stats()
    assert isinstance(s, CredentialStats)
    assert s.secret_count == 0
    assert s.last_sync_age_ms is None
    assert s.sync_failures == 0
    assert s.rotations == 0

    c.put("a", b"1")
    c.put("b", b"2")
    c.put("c", b"3")
    s = c.stats()
    assert s.secret_count == 3
    # No central sync configured → no sync age / failures / rotations.
    assert s.last_sync_age_ms is None
    assert s.sync_failures == 0
    assert s.rotations == 0


@pytest.mark.skipif(os.environ.get("GGCOMMONS_IT_KMS") != "1", reason="needs floci KMS (GGCOMMONS_IT_KMS=1)")
def test_kms_key_provider_roundtrip(tmp_path):
    import boto3

    from ggcommons.credentials import open_from_config

    os.environ.setdefault("AWS_ACCESS_KEY_ID", "test")
    os.environ.setdefault("AWS_SECRET_ACCESS_KEY", "test")
    os.environ.setdefault("AWS_REGION", "us-east-1")

    kms = boto3.client("kms", region_name="us-east-1", endpoint_url="http://localhost:4566")
    key_id = kms.create_key()["KeyMetadata"]["KeyId"]

    cfg = {
        "vault": {
            "path": str(tmp_path / "vault"),
            "keyProvider": {
                "type": "kms",
                "kmsKeyId": key_id,
                "region": "us-east-1",
                "endpointUrl": "http://localhost:4566",
            },
        },
    }
    creds = open_from_config(cfg)
    creds.put("db/password", b"s3cr3t")

    # Reopen from disk — forces a fresh kms:Decrypt to unwrap the DEK.
    creds2 = open_from_config(cfg)
    assert creds2.get_string("db/password") == "s3cr3t"


@pytest.mark.skipif(os.environ.get("GGCOMMONS_IT_PKCS11") != "1", reason="needs a PKCS#11 token (GGCOMMONS_IT_PKCS11=1)")
def test_pkcs11_key_provider_roundtrip(tmp_path):
    """PKCS#11 round-trip against a real token (e.g. SoftHSM2). Env: PKCS11_MODULE/TOKEN/KEY/PIN."""
    from ggcommons.credentials import open_from_config

    cfg = {
        "vault": {
            "path": str(tmp_path / "vault"),
            "keyProvider": {
                "type": "pkcs11",
                "modulePath": os.environ["PKCS11_MODULE"],
                "tokenLabel": os.environ["PKCS11_TOKEN"],
                "keyLabel": os.environ["PKCS11_KEY"],
                "pin": os.environ["PKCS11_PIN"],
            },
        },
    }
    creds = open_from_config(cfg)
    creds.put("db/password", b"s3cr3t")

    # Reopen from disk — forces a fresh HSM unwrap of the DEK (fail-closed otherwise).
    creds2 = open_from_config(cfg)
    assert creds2.get_string("db/password") == "s3cr3t"


@pytest.mark.skipif(not (VECTORS_DIR / "vault.json").exists(), reason="vault-test-vectors not present")
def test_cross_language_conformance():
    """Decrypt the canonical vault and reproduce ciphertext/wrappedDek/MAC from the fixed inputs."""
    vec = json.loads((VECTORS_DIR / "vectors.json").read_text())
    kek = base64.b64decode(vec["kekB64"])
    dek = base64.b64decode(vec["dekB64"])
    vault_id = vec["vaultId"]

    # (1) decrypt the canonical vault using the committed key file
    provider = FileKeyProvider.from_keyfile(str(VECTORS_DIR / "vault.key"))
    v = LocalVault.open(str(VECTORS_DIR / "vault.json"), provider, 2)
    assert v.get("alpha").bytes() == b"hello"
    assert v.get("beta").as_json()["x"] == 1

    # (2) reproduce the wrapped DEK
    wrapped = crypto.seal(kek, base64.b64decode(vec["wrapNonceB64"]), fmt.dek_wrap_aad(vault_id), dek)
    assert base64.b64encode(wrapped).decode() == vec["wrappedDekB64"]

    # (3) reproduce each record ciphertext
    secrets = {}
    for r in vec["records"]:
        nonce = base64.b64decode(r["nonceB64"])
        pt = base64.b64decode(r["plaintextB64"])
        ct = crypto.seal(dek, nonce, fmt.record_aad(vault_id, r["name"], r["version"]), pt)
        assert base64.b64encode(ct).decode() == r["ciphertextB64"], r["name"]
        secrets[r["name"]] = {"versions": [{
            "version": r["version"], "createdMs": 1_700_000_000_000, "source": "local",
            "contentType": "application/octet-stream",
            "nonce": r["nonceB64"], "ciphertext": r["ciphertextB64"],
        }]}

    # (4) reproduce the MAC over the canonical byte string
    mac_key = crypto.derive_mac_key(dek, vault_id)
    mac = base64.b64encode(
        crypto.hmac_sha256(mac_key, fmt.mac_input(vault_id, secrets, base64.b64decode))
    ).decode()
    assert mac == vec["macB64"]
