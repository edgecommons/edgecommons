"""Parse the ``credentials`` config section and build a service.

Phase 1: ``file`` key provider + local vault. Phase 2: ``awsSecretsManager`` central source + sync.
``namespace`` (``<thingName>/<componentName>``) is applied transparently to every key.
"""
import os
import threading

from .audit import log_sink
from .errors import CredentialError
from .keyprovider import EnvKeyProvider, FileKeyProvider, KmsKeyProvider, Pkcs11KeyProvider
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


def build_key_provider(kp: dict, default_key_path: str, default_type: str = "file"):
    """Build a :class:`KeyProvider` from a ``keyProvider`` config dict.

    ``kp.type`` is one of ``file`` (default), ``env``, ``kms``/``greengrass``, or ``pkcs11``.
    ``default_key_path`` is the file-key path used when the ``file`` provider does not set ``keyPath``.
    ``default_type`` is the provider type used when ``kp.type`` is absent/unspecified — the middle
    precedence tier (FR-RT-3): the credentials init site passes the platform-profile default (``env``
    on KUBERNETES) here, falling back to the library default ``file``. Shared by the credentials vault
    and the parameters persistent cache so both honour the same key-provider config.
    """
    kp = kp or {}
    kind = kp.get("type") or default_type or "file"
    if kind == "file":
        key_path = kp.get("keyPath") or default_key_path
        parent = os.path.dirname(os.path.abspath(key_path))
        os.makedirs(parent, exist_ok=True)
        return (FileKeyProvider.from_keyfile(key_path)
                if os.path.exists(key_path)
                else FileKeyProvider.generate_keyfile(key_path))
    if kind == "env":
        return EnvKeyProvider(kp.get("envVar") or EnvKeyProvider.DEFAULT_ENV_VAR)
    if kind in ("kms", "greengrass"):
        key_id = kp.get("kmsKeyId")
        if not key_id:
            raise CredentialError("kms key provider requires keyProvider.kmsKeyId")
        return KmsKeyProvider(key_id, region=kp.get("region"), endpoint_url=kp.get("endpointUrl"))
    if kind == "pkcs11":
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
        return Pkcs11KeyProvider(module_path, kp.get("tokenLabel", ""), key_label, pin)
    raise CredentialError(
        f"key provider '{kind}' is not supported "
        "(supported: 'file', 'env', 'kms'/'greengrass', 'pkcs11')"
    )


def open_from_config(
    credentials_cfg: dict,
    namespace: str = "",
    default_key_provider: str = None,
) -> DefaultCredentialService:
    """Open the vault and return the default credential service from a ``credentials`` config dict.

    ``namespace`` is prepended transparently to every key (typically ``<thing>/<component>``).
    ``default_key_provider`` is the platform-profile default key-provider type (FR-CRED-6 / FR-RT-3):
    ``env`` on KUBERNETES, ``None`` elsewhere. It is used **only** when ``keyProvider.type`` is absent
    (explicit type always wins; ``None`` falls through to the library default ``file``). It does not
    affect whether credentials is enabled — that is gated solely by the presence of a ``credentials``
    config section at the call site.
    """
    cfg = credentials_cfg or {}
    vault_cfg = cfg.get("vault", {})
    path = vault_cfg.get("path", "vault")
    keep_versions = int(vault_cfg.get("keepVersions", 2))
    provider = build_key_provider(
        vault_cfg.get("keyProvider", {}) or {},
        f"{path}.key",
        default_type=default_key_provider or "file",
    )

    vault = LocalVault.open(path, provider, keep_versions)
    lock = threading.Lock()

    # Access auditing on by default (config can disable) — logs op/name/version/source/outcome,
    # never the value.
    audit_cfg = cfg.get("audit", {}) or {}
    audit = log_sink() if audit_cfg.get("enabled", True) else None

    central = cfg.get("central", {}) or {}
    ctype = central.get("type", "none")
    if ctype == "none":
        return DefaultCredentialService(vault, namespace=namespace, lock=lock, audit=audit)
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
    return DefaultCredentialService(vault, namespace=namespace, sync=engine, lock=lock, audit=audit)
