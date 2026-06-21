"""Credentials & local vault — a generic encrypted-at-rest secret store for Python components.

A peer subsystem to config/messaging/metrics. Named, versioned, opaque-byte secrets in an
encrypted local vault that runs standalone or (later phases) is seeded/refreshed from a central
cloud vault. The on-disk format is byte-compatible with the Java/Rust/TS ports
(see ``vault-test-vectors/`` and ``docs/CREDENTIALS.md``).

Example::

    from ggcommons.credentials import open_from_config
    creds = open_from_config({"vault": {"path": "/var/lib/ggcommons/vault"}})
    creds.put("db/password", b"s3cr3t")
    pw = creds.get_string("db/password")
"""
from .central import AwsSecretsManagerSource, CentralSecret, CentralVaultSource
from .config import open_from_config
from .errors import CredentialError
from .keyprovider import FileKeyProvider, KeyProvider
from .service import CredentialService, DefaultCredentialService, Secret, SecretMeta
from .sync import SyncEngine
from .vault import LocalVault
from .views import AwsCredentials, BasicAuth, KafkaSasl, TlsBundle

__all__ = [
    "open_from_config",
    "CredentialError",
    "KeyProvider",
    "FileKeyProvider",
    "CredentialService",
    "DefaultCredentialService",
    "Secret",
    "SecretMeta",
    "LocalVault",
    "CentralVaultSource",
    "CentralSecret",
    "AwsSecretsManagerSource",
    "SyncEngine",
    "AwsCredentials",
    "BasicAuth",
    "TlsBundle",
    "KafkaSasl",
]
