"""Local vault — the encrypted-at-rest secret store (Python port of the Rust reference).

Single JSON file; AES-256-GCM records; envelope-wrapped DEK; HMAC over the canonical byte string.
Atomic ``os.replace`` writes under a cross-process advisory lock (``filelock``) for the shared
device vault; reload-on-change for cross-process read freshness; fail-closed on bad KEK/tamper.
"""
import base64
import json
import os
import time
import uuid
from typing import Dict, List, Optional

from filelock import FileLock

from . import crypto, format as fmt
from .errors import CredentialError
from .keyprovider import KeyProvider
from .service import Secret, SecretMeta

_FORMAT = fmt.FORMAT_VERSION


def _b64e(b: bytes) -> str:
    return base64.b64encode(b).decode("ascii")


def _b64d(s: str) -> bytes:
    return base64.b64decode(s)


def _now_ms() -> int:
    return int(time.time() * 1000)


class LocalVault:
    """The encrypted local secret store."""

    def __init__(self, path: str, vault_id: str, dek: bytes, key_provider: KeyProvider,
                 kek: dict, secrets: Dict[str, dict], keep_versions: int):
        self._path = path
        self._vault_id = vault_id
        self._dek = dek
        self._key_provider = key_provider  # retained for phase-2 KEK rotation
        self._kek = kek
        self._secrets = secrets
        self._keep = max(1, keep_versions)
        self._stamp = self._file_stamp()

    # ----- open / create -----
    @classmethod
    def open(cls, path: str, key_provider: KeyProvider, keep_versions: int = 2) -> "LocalVault":
        if os.path.exists(path):
            vf = _read_file(path)
            if vf.get("format") != _FORMAT:
                raise CredentialError(f"unsupported vault format {vf.get('format')}")
            vault_id = vf["vaultId"]
            dek = key_provider.unwrap_dek(vault_id, vf["kek"])
            _verify_mac(dek, vf)
            return cls(path, vault_id, dek, key_provider, vf["kek"], vf.get("secrets", {}), keep_versions)
        # create fresh
        parent = os.path.dirname(os.path.abspath(path))
        os.makedirs(parent, exist_ok=True)
        vault_id = str(uuid.uuid4())
        dek = crypto.random(crypto.KEY_LEN)
        kek = key_provider.wrap_dek(vault_id, dek)
        v = cls(path, vault_id, dek, key_provider, kek, {}, keep_versions)
        v._save()
        return v

    @property
    def vault_id(self) -> str:
        return self._vault_id

    # ----- reads -----
    def get(self, name: str) -> Optional[Secret]:
        entry = self._secrets.get(name)
        if not entry or not entry["versions"]:
            return None
        return self._decrypt(name, entry["versions"][-1])

    def get_version(self, name: str, version: str) -> Optional[Secret]:
        entry = self._secrets.get(name)
        if not entry:
            return None
        for v in entry["versions"]:
            if v["version"] == version:
                return self._decrypt(name, v)
        return None

    def exists(self, name: str) -> bool:
        entry = self._secrets.get(name)
        return bool(entry and entry["versions"])

    def list(self, prefix: str = "") -> List[SecretMeta]:
        out = []
        for name in sorted(self._secrets, key=lambda n: n.encode("utf-8")):
            if not name.startswith(prefix):
                continue
            versions = self._secrets[name]["versions"]
            if versions:
                out.append(_meta_of(name, versions[-1]))
        return out

    def versions(self, name: str) -> List[str]:
        entry = self._secrets.get(name)
        return [v["version"] for v in entry["versions"]] if entry else []

    # ----- writes -----
    def put(self, name: str, plaintext: bytes, ttl_secs=None, labels=None,
            content_type=None, source=None, central_version_id=None) -> str:
        version = self._next_version(name)
        nonce = crypto.random(crypto.NONCE_LEN)
        ct = crypto.seal(self._dek, nonce, fmt.record_aad(self._vault_id, name, version), plaintext)
        entry = self._secrets.setdefault(name, {"versions": []})
        rec = {
            "version": version,
            "createdMs": _now_ms(),
            "source": source or "local",
            "contentType": content_type or "application/octet-stream",
            "nonce": _b64e(nonce),
            "ciphertext": _b64e(ct),
        }
        if ttl_secs is not None:
            rec["ttlSecs"] = int(ttl_secs)
        if labels:
            rec["labels"] = dict(labels)
        if central_version_id is not None:
            rec["centralVersionId"] = central_version_id
        entry["versions"].append(rec)
        if len(entry["versions"]) > self._keep:
            del entry["versions"][0:len(entry["versions"]) - self._keep]
        self._save()
        return version

    def delete(self, name: str) -> bool:
        if name in self._secrets:
            del self._secrets[name]
            self._save()
            return True
        return False

    def reload_if_changed(self) -> bool:
        cur = self._file_stamp()
        if cur == self._stamp:
            return False
        vf = _read_file(self._path)
        _verify_mac(self._dek, vf)
        self._secrets = vf.get("secrets", {})
        self._kek = vf["kek"]
        self._stamp = cur
        return True

    # ----- internals -----
    def _next_version(self, name: str) -> str:
        entry = self._secrets.get(name)
        n = 0
        if entry and entry["versions"]:
            try:
                n = int(entry["versions"][-1]["version"])
            except ValueError:
                n = 0
        return f"{n + 1:08d}"

    def _decrypt(self, name: str, v: dict) -> Secret:
        nonce = _b64d(v["nonce"])
        ct = _b64d(v["ciphertext"])
        aad = fmt.record_aad(self._vault_id, name, v["version"])
        plaintext = crypto.open_(self._dek, nonce, aad, ct)
        return Secret(
            name=name, version=v["version"], _bytes=plaintext,
            labels=dict(v.get("labels", {})), created_ms=int(v.get("createdMs", 0)),
            source=v.get("source", "local"), content_type=v.get("contentType", "application/octet-stream"),
        )

    def _save(self):
        mac_key = crypto.derive_mac_key(self._dek, self._vault_id)
        mac = _b64e(crypto.hmac_sha256(mac_key, fmt.mac_input(self._vault_id, self._secrets, _b64d)))
        vf = {
            "format": _FORMAT,
            "vaultId": self._vault_id,
            "kek": self._kek,
            "secrets": self._secrets,
            "mac": mac,
        }
        data = json.dumps(vf, indent=2).encode("utf-8")
        with FileLock(self._path + ".lock"):
            tmp = self._path + ".tmp"
            with open(tmp, "wb") as f:
                f.write(data)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, self._path)
        self._stamp = self._file_stamp()

    def _file_stamp(self):
        try:
            st = os.stat(self._path)
            return (st.st_mtime_ns, st.st_size)
        except OSError:
            return None


def _read_file(path: str) -> dict:
    with open(path, "rb") as f:
        return json.loads(f.read())


def _verify_mac(dek: bytes, vf: dict):
    mac_key = crypto.derive_mac_key(dek, vf["vaultId"])
    expected = _b64d(vf["mac"])
    inp = fmt.mac_input(vf["vaultId"], vf.get("secrets", {}), _b64d)
    if not crypto.hmac_verify(mac_key, inp, expected):
        raise CredentialError("vault integrity check failed (tampered or wrong key)")


def _meta_of(name: str, v: dict) -> SecretMeta:
    return SecretMeta(
        name=name, version=v["version"], created_ms=int(v.get("createdMs", 0)),
        ttl_secs=v.get("ttlSecs"), source=v.get("source", "local"), labels=dict(v.get("labels", {})),
    )
