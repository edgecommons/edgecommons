"""Vault on-disk format helpers (normative; must match the Rust reference).

The AEAD AADs and the length-prefixed canonical MAC input are defined here. The MAC is taken
over this byte string (not the JSON text) so JSON formatting may differ between languages while
the integrity check stays identical. See ``docs/CREDENTIALS.md`` §4.
"""
import struct
from typing import Callable, Dict

FORMAT_VERSION = 1


def record_aad(vault_id: str, name: str, version: str) -> bytes:
    """AEAD AAD binding a record to its vault, name, and version."""
    return f"ggcommons-vault/v1|{vault_id}|{name}|{version}".encode("utf-8")


def dek_wrap_aad(vault_id: str) -> bytes:
    """AEAD AAD binding the wrapped DEK to its vault."""
    return f"ggcommons-vault/v1/dek-wrap|{vault_id}".encode("utf-8")


def _lp(b: bytes) -> bytes:
    """Length-prefix: ``u32_le(len) || b``."""
    return struct.pack("<I", len(b)) + b


def mac_input(vault_id: str, secrets: Dict[str, dict], decode_b64: Callable[[str], bytes]) -> bytes:
    """Build the canonical MAC input over the whole secret set.

    Layout (little-endian; ``lp(x) = u32_le(len) || x``)::

        b"ggcommons-vault/v1/mac"
          || lp(vaultId)
          || u32_le(secret_count)
          || for each secret (sorted by name UTF-8 bytes):
               lp(name) || u32_le(version_count)
                 || for each version (array order):
                     lp(version) || u64_le(createdMs) || u64_le(ttlSecs or 0)
                     || lp(source) || lp(centralVersionId or "")
                     || lp(nonce_raw) || lp(ciphertext_raw)
    """
    out = bytearray(b"ggcommons-vault/v1/mac")
    out += _lp(vault_id.encode("utf-8"))
    items = sorted(secrets.items(), key=lambda kv: kv[0].encode("utf-8"))
    out += struct.pack("<I", len(items))
    for name, entry in items:
        out += _lp(name.encode("utf-8"))
        versions = entry["versions"]
        out += struct.pack("<I", len(versions))
        for v in versions:
            out += _lp(v["version"].encode("utf-8"))
            out += struct.pack("<Q", int(v.get("createdMs", 0)))
            out += struct.pack("<Q", int(v.get("ttlSecs") or 0))
            out += _lp(v.get("source", "").encode("utf-8"))
            out += _lp((v.get("centralVersionId") or "").encode("utf-8"))
            out += _lp(decode_b64(v["nonce"]))
            out += _lp(decode_b64(v["ciphertext"]))
    return bytes(out)
