"""Parse the ``credentials`` config section and build a service.

Phase 1: ``file`` key provider + ``central.type: none``. Other providers/sources raise a clear
"phase 2" error.
"""
import os

from .errors import CredentialError
from .keyprovider import FileKeyProvider
from .service import DefaultCredentialService
from .vault import LocalVault


def open_from_config(credentials_cfg: dict) -> DefaultCredentialService:
    """Open the vault and return the default credential service from a ``credentials`` config dict."""
    cfg = credentials_cfg or {}
    vault_cfg = cfg.get("vault", {})
    path = vault_cfg.get("path", "vault")
    keep_versions = int(vault_cfg.get("keepVersions", 2))
    kp = vault_cfg.get("keyProvider", {}) or {}
    kind = kp.get("type", "file")

    if kind != "file":
        raise CredentialError(f"key provider '{kind}' is not implemented yet (phase 1 supports 'file')")

    central = (cfg.get("central", {}) or {}).get("type", "none")
    if central != "none":
        raise CredentialError(f"central source '{central}' is not implemented yet (phase 2)")

    key_path = kp.get("keyPath") or f"{path}.key"
    parent = os.path.dirname(os.path.abspath(key_path))
    os.makedirs(parent, exist_ok=True)
    provider = (FileKeyProvider.from_keyfile(key_path)
                if os.path.exists(key_path)
                else FileKeyProvider.generate_keyfile(key_path))

    return DefaultCredentialService(LocalVault.open(path, provider, keep_versions))
