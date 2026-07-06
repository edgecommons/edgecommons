"""Typed credential views — thin parses over an opaque Secret (canonical camelCase JSON).

- AWS creds: {accessKeyId, secretAccessKey, sessionToken?, expiry?}
- basic auth: {username, password}
- TLS bundle: {certPem, keyPem, caPem?}
- Kafka SASL: {mechanism?, username, password}  (mechanism defaults to PLAIN)
"""
from dataclasses import dataclass
from typing import Optional

from .errors import CredentialError


@dataclass
class AwsCredentials:
    access_key_id: str
    secret_access_key: str
    session_token: Optional[str] = None
    expiry: Optional[str] = None


@dataclass
class BasicAuth:
    username: str
    password: str


@dataclass
class TlsBundle:
    cert_pem: str
    key_pem: str
    ca_pem: Optional[str] = None


@dataclass
class KafkaSasl:
    username: str
    password: str
    mechanism: str = "PLAIN"


def _require(d: dict, key: str, kind: str):
    try:
        return d[key]
    except (KeyError, TypeError):
        raise CredentialError(f"secret is not {kind} (missing '{key}')") from None
