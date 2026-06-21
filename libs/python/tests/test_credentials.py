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
