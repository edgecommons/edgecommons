"""Secret references (``$secret``) in config.

Let any subsystem's config point at a vault secret instead of embedding the value —
``{"$secret": "name"}`` (whole value) or ``{"$secret": "name", "field": "key"}`` (a field of the
secret's JSON). Resolved lazily at subsystem-init time so the secret never lands in the
logged/templated config snapshot. This is how streaming/messaging consume credentials. Mirrors the
Rust reference (``secretref.rs``).
"""
from typing import Optional

from .errors import CredentialError


def resolve_secret_refs(value, creds) -> None:
    """Recursively replace ``$secret`` references in ``value`` (a parsed JSON dict/list), in place.

    An object ``{"$secret": "name"}`` is replaced with the secret's string value;
    ``{"$secret": "name", "field": "k"}`` with field ``k`` of the secret parsed as JSON.

    Raises :class:`CredentialError` if a referenced secret (or requested field) is absent.
    """
    if isinstance(value, dict):
        ref = value.get("$secret")
        if isinstance(ref, str):
            field = value.get("field")
            field = field if isinstance(field, str) else None
            resolved = _resolve_one(ref, field, creds)
            value.clear()
            return resolved
        for k in list(value.keys()):
            r = resolve_secret_refs(value[k], creds)
            if r is not None:
                value[k] = r
    elif isinstance(value, list):
        for i, item in enumerate(value):
            r = resolve_secret_refs(item, creds)
            if r is not None:
                value[i] = r
    return None


def _resolve_one(name: str, field: Optional[str], creds) -> str:
    if not creds.exists(name):
        raise CredentialError(f"secretRef '{name}' not found in the vault")
    if field is None:
        s = creds.get_string(name)
        if s is None:
            raise CredentialError(f"secretRef '{name}' not found in the vault")
        return s
    d = creds.get_json(name)
    if d is None:
        raise CredentialError(f"secretRef '{name}' not found in the vault")
    v = d.get(field) if isinstance(d, dict) else None
    if not isinstance(v, str):
        raise CredentialError(f"secretRef '{name}' field '{field}' missing or not a string")
    return v
