"""AWS SSM Parameter Store source.

Reads parameters from AWS SSM via ``GetParameter`` / ``GetParametersByPath`` (with decryption, so
``SecureString``s resolve and are flagged ``secure``). Uses the default credential chain — TES on
Greengrass, ambient creds in STANDALONE. ``boto3`` is imported lazily so ``env`` / ``mountedDir``
work with no AWS SDK installed.

Mirrors the Rust reference (``libs/rust/src/parameters/ssm.rs``).
"""
from typing import List, Optional, Tuple

from .errors import ParameterError
from .source import ParamValue, ParameterSource


class AwsSsmSource(ParameterSource):
    """AWS SSM Parameter Store :class:`ParameterSource`."""

    def __init__(self, region: Optional[str] = None, endpoint_url: Optional[str] = None,
                 with_decryption: bool = True):
        try:
            import boto3  # lazy: only needed for the SSM source
        except ImportError as e:
            raise ParameterError(
                "awsSsm parameter source requires boto3 (pip install boto3)"
            ) from e
        self._with_decryption = with_decryption
        kwargs = {}
        if region:
            kwargs["region_name"] = region
        if endpoint_url:
            kwargs["endpoint_url"] = endpoint_url
        self._client = boto3.client("ssm", **kwargs)

    @staticmethod
    def _to_value(p: dict) -> Optional[ParamValue]:
        value = p.get("Value")
        if value is None:
            return None
        secure = p.get("Type") == "SecureString"
        version = p.get("Version")
        return ParamValue(
            value.encode("utf-8"),
            secure=secure,
            version=str(version) if version is not None else None,
        )

    def fetch(self, name: str) -> Optional[ParamValue]:
        try:
            resp = self._client.get_parameter(Name=name, WithDecryption=self._with_decryption)
        except self._client.exceptions.ParameterNotFound:
            return None
        except Exception as e:
            raise ParameterError(f"ssm get_parameter: {e}") from None
        p = resp.get("Parameter")
        return self._to_value(p) if p else None

    def fetch_by_path(self, path: str, recursive: bool) -> List[Tuple[str, ParamValue]]:
        out: List[Tuple[str, ParamValue]] = []
        next_token: Optional[str] = None
        while True:
            kwargs = {
                "Path": path,
                "Recursive": recursive,
                "WithDecryption": self._with_decryption,
            }
            if next_token:
                kwargs["NextToken"] = next_token
            try:
                resp = self._client.get_parameters_by_path(**kwargs)
            except Exception as e:
                raise ParameterError(f"ssm get_parameters_by_path: {e}") from None
            for p in resp.get("Parameters", []):
                name = p.get("Name")
                v = self._to_value(p)
                if name and v is not None:
                    out.append((name, v))
            next_token = resp.get("NextToken")
            if not next_token:
                break
        return out

    def source_id(self) -> str:
        return "awsSsm"
