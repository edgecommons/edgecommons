"""Sync engine — seed + refresh the local vault from a central source. Offline-first, selective,
rotation-aware. Mirrors the Rust reference."""
import logging
import threading
from typing import List, Optional, Tuple

from .central import CentralVaultSource

logger = logging.getLogger("ggcommons.credentials.sync")


class SyncEngine:
    """Owns the background refresh thread (daemon); stops on :meth:`close`."""

    def __init__(
        self,
        vault,
        lock: threading.Lock,
        source: CentralVaultSource,
        namespace: str,
        secrets: List[Tuple[str, Optional[str]]],
        interval_secs: int,
        bootstrap: bool,
    ):
        self._vault = vault
        self._lock = lock
        self._source = source
        self._namespace = namespace
        self._secrets = secrets  # (caller_name, central_id_override)
        self._stop = threading.Event()
        self._thread = None
        if bootstrap:
            self.sync_now()
        if interval_secs > 0:
            self._thread = threading.Thread(target=self._loop, args=(interval_secs,), daemon=True)
            self._thread.start()

    def _local_key(self, name: str) -> str:
        return f"{self._namespace}/{name}" if self._namespace else name

    def sync_now(self) -> None:
        for name, override in self._secrets:
            local_key = self._local_key(name)
            # Central id defaults to the namespaced path (per-device); override = shared/fleet id.
            central_id = override or local_key
            try:
                cs = self._source.fetch(central_id)
            except Exception as e:  # offline-first: keep the cached value
                logger.warning(f"central fetch failed for '{central_id}'; using cached value: {e}")
                continue
            if cs is None:
                continue
            with self._lock:
                self._vault.reload_if_changed()
                if self._vault.latest_central_version_id(local_key) == cs.central_version_id:
                    continue
                self._vault.put(
                    local_key, cs.bytes, source="central",
                    central_version_id=cs.central_version_id, labels=cs.labels,
                )
                logger.info(f"secret '{local_key}' synced from central ({central_id})")

    def _loop(self, interval_secs: int) -> None:
        while not self._stop.wait(interval_secs):
            self.sync_now()

    def close(self) -> None:
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=2)
