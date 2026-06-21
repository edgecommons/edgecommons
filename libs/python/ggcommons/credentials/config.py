"""Parse the ``credentials`` config section and build a service.

Phase 1: ``file`` key provider + local vault. Phase 2: ``awsSecretsManager`` central source + sync.
``namespace`` (``<thingName>/<componentName>``) is applied transparently to every key.
"""
import os
import threading

from .errors import CredentialError
from .keyprovider import FileKeyProvider, KmsKeyProvider, Pkcs11KeyProvider
from .service import DefaultCredentialService
from .vault import LocalVault


def _sync_entries(sync_cfg: dict):
    """Normalize sync.secrets entries to (caller_name, central_id_override) tuples."""
    out = []
    for entry in (sync_cfg or {}).get("secrets", []) or []:
        if isinstance(entry, str):
            out.append((entry, None))
        elif isinstance(entry, dict) and "name" in entry:
            out.append((entry["name"], entry.get("from")))
    return out


def open_from_config(credentials_cfg: dict, namespace: str = "") -> DefaultCredentialService:
    """Open the vault and return the default credential service from a ``credentials`` config dict.

    ``namespace`` is prepended transparently to every key (typically ``<thing>/<component>``).
    """
    cfg = credentials_cfg or {}
    vault_cfg = cfg.get("vault", {})
    path = vault_cfg.get("path", "vault")
    keep_versions = int(vault_cfg.get("keepVersions", 2))
    kp = vault_cfg.get("keyProvider", {}) or {}
    kind = kp.get("type", "file")
    if kind == "file":
        key_path = kp.get("keyPath") or f"{path}.key"
        parent = os.path.dirname(os.path.abspath(key_path))
        os.makedirs(parent, exist_ok=True)
        provider = (FileKeyProvider.from_keyfile(key_path)
                    if os.path.exists(key_path)
                    else FileKeyProvider.generate_keyfile(key_path))
    elif kind in ("kms", "greengrass"):
        key_id = kp.get("kmsKeyId")
        if not key_id:
            raise CredentialError("kms key provider requires keyProvider.kmsKeyId")
        provider = KmsKeyProvider(key_id, region=kp.get("region"), endpoint_url=kp.get("endpointUrl"))
    elif kind == "pkcs11":
        module_path = kp.get("modulePath")
        if not module_path:
            raise CredentialError("pkcs11 key provider requires keyProvider.modulePath")
        key_label = kp.get("keyLabel")
        if not key_label:
            raise CredentialError("pkcs11 key provider requires keyProvider.keyLabel")
        pin_env = kp.get("pinEnv")
        if pin_env:
            pin = os.environ.get(pin_env)
            if pin is None:
                raise CredentialError(f"pkcs11 keyProvider.pinEnv '{pin_env}' is not set")
        elif kp.get("pin") is not None:
            pin = kp.get("pin")
        else:
            raise CredentialError("pkcs11 key provider requires keyProvider.pinEnv or keyProvider.pin")
        provider = Pkcs11KeyProvider(module_path, kp.get("tokenLabel", ""), key_label, pin)
    else:
        raise CredentialError(
            f"key provider '{kind}' is not supported (supported: 'file', 'kms'/'greengrass', 'pkcs11')"
        )

    vault = LocalVault.open(path, provider, keep_versions)
    lock = threading.Lock()

    central = cfg.get("central", {}) or {}
    ctype = central.get("type", "none")
    if ctype == "none":
        return DefaultCredentialService(vault, namespace=namespace, lock=lock)
    if ctype != "awsSecretsManager":
        raise CredentialError(f"central source '{ctype}' is not supported")

    from .central import AwsSecretsManagerSource
    from .sync import SyncEngine

    source = AwsSecretsManagerSource(region=central.get("region"), endpoint_url=central.get("endpointUrl"))
    engine = SyncEngine(
        vault, lock, source, namespace,
        _sync_entries(central.get("sync", {})),
        int(central.get("refreshIntervalSecs", 300)),
        bool(central.get("bootstrapOnStart", True)),
    )
    return DefaultCredentialService(vault, namespace=namespace, sync=engine, lock=lock)
