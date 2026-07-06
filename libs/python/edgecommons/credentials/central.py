"""Central vault sources — the upstream a vault is seeded/refreshed from (AWS Secrets Manager)."""
from abc import ABC, abstractmethod
from dataclasses import dataclass, field
from typing import Dict, Optional

from .errors import CredentialError


@dataclass
class CentralSecret:
    """A secret value fetched from the central source."""
    bytes: bytes
    central_version_id: str
    labels: Dict[str, str] = field(default_factory=dict)


class CentralVaultSource(ABC):
    """The upstream source a vault syncs from."""

    @abstractmethod
    def fetch(self, name: str) -> Optional[CentralSecret]:
        """Fetch the current value of ``name``, or ``None`` if it does not exist upstream."""


class AwsSecretsManagerSource(CentralVaultSource):
    """Central source backed by AWS Secrets Manager (boto3). Auth = default chain (TES on
    Greengrass); ``endpoint_url`` overrides for an emulator (floci/LocalStack) or VPC endpoint."""

    def __init__(self, region: Optional[str] = None, endpoint_url: Optional[str] = None):
        import boto3  # imported lazily so non-sync components don't require it at import time

        self._client = boto3.client("secretsmanager", region_name=region, endpoint_url=endpoint_url)

    def fetch(self, name: str) -> Optional[CentralSecret]:
        try:
            r = self._client.get_secret_value(SecretId=name)
        except self._client.exceptions.ResourceNotFoundException:
            return None
        except Exception as e:  # connectivity/auth/etc. — surfaced; the sync engine keeps cache
            raise CredentialError(f"get secret '{name}': {e}") from None
        if "SecretString" in r and r["SecretString"] is not None:
            data = r["SecretString"].encode("utf-8")
        elif "SecretBinary" in r and r["SecretBinary"] is not None:
            data = bytes(r["SecretBinary"])
        else:
            return None
        return CentralSecret(bytes=data, central_version_id=r.get("VersionId", ""), labels={})
