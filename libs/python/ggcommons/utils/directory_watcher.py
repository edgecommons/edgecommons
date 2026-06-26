"""A directory watcher built for the kubelet's atomic ConfigMap update mechanism.

Watches a *directory* (not a single file inode) and fires a callback on *any* create/modify/delete of
an entry within it. This is the Kubernetes-aware sibling of the FILE source's file observer, built for
the kubelet's atomic ConfigMap update mechanism (DESIGN-subsystems sec 1, FR-CFG-2).

A mounted ConfigMap is a directory of symlinks: the user-visible ``config.json`` points at
``..data/config.json``, and ``..data`` is itself a symlink the kubelet swaps atomically (write a new
timestamped dir, create ``..data_tmp`` pointing at it, then ``rename(..data_tmp, ..data)``).
Crucially:

* a watch on the user-visible *file* fires once and dies after the swap (the inode it pointed at is
  gone — ``IN_DELETE_SELF``); and
* the swap manifests as events on the ``..data`` / ``..data_tmp`` entries, *not* on ``config.json``,
  so a name-filtered file watch never reloads.

Therefore this watcher (a) watches the mount directory, which persists across swaps; (b) reacts to
*every* entry change so the ``..data`` swap triggers a reload; and (c) **re-arms** — it re-scans the
directory each poll cycle, so the watch survives inode replacement / symlink swap rather than silently
going dead.

Mirrors the canonical Java ``com.mbreissi.ggcommons.utils.DirectoryWatcher``. Java uses the JDK
``WatchService`` with an explicit re-arm loop; Python's portable, deterministic equivalent is a
periodic re-scan poll loop, which is inherently immune to inode replacement (it holds no inode/key
across cycles). The dotfile filter that prevents the projection artifacts from being *parsed* as
config lives in the provider; this watcher intentionally does not filter events, because the
``..data`` swap is exactly the signal it must act on.
"""

import logging
import os
import threading
from typing import Callable, Optional, Set, Tuple

logger = logging.getLogger("DirectoryWatcher")

#: Default poll interval (seconds) for the re-scan loop.
DEFAULT_POLL_INTERVAL = 0.2


class DirectoryWatcher(threading.Thread):
    """A daemon thread that watches a directory and invokes ``handler`` on any entry change.

    The watcher re-scans the directory each poll cycle and fires when the snapshot of its entries
    (names + size + modification time, via ``lstat`` so a swapped ``..data`` symlink is detected as
    itself changing) differs from the previous cycle. Re-scanning means it naturally re-arms across an
    atomic ``..data`` swap or any file-inode replacement, and it tolerates the watched directory not
    existing yet (it begins delivering events once it appears).

    Args:
        directory: the directory to watch (e.g. the ConfigMap mount point).
        handler: the callback invoked when any entry in ``directory`` changes.
        poll_interval: seconds between re-scans (default :data:`DEFAULT_POLL_INTERVAL`).
    """

    def __init__(
        self,
        directory: str,
        handler: Callable[[], None],
        poll_interval: float = DEFAULT_POLL_INTERVAL,
    ):
        super().__init__(name=f"DirectoryWatcher-{os.path.basename(directory)}", daemon=True)
        self._directory = directory
        self._handler = handler
        self._poll_interval = poll_interval
        self._stop_event = threading.Event()

    def is_stopped(self) -> bool:
        """Return ``True`` once :meth:`stop_thread` has been called."""
        return self._stop_event.is_set()

    def stop_thread(self) -> None:
        """Signal the watcher thread to exit and wake it from its poll wait."""
        self._stop_event.set()

    def do_on_change(self) -> None:
        """Execute the change handler callback. Invoked internally when directory changes are detected."""
        self._handler()

    def _signature(self) -> Optional[Set[Tuple[str, int, int]]]:
        """Snapshot the directory entries as ``(name, size, mtime_ns)`` triples.

        Uses ``lstat`` so that an atomic swap of the ``..data`` symlink (a new inode with a fresh
        mtime) registers as a change. Returns ``None`` if the directory does not currently exist (a
        swap window or a not-yet-mounted volume), which the loop treats as "no readable state".
        """
        try:
            sig: Set[Tuple[str, int, int]] = set()
            with os.scandir(self._directory) as it:
                for entry in it:
                    try:
                        st = entry.stat(follow_symlinks=False)
                        sig.add((entry.name, st.st_size, st.st_mtime_ns))
                    except OSError:
                        # Entry vanished mid-scan (a swap window) — treat as a change next cycle.
                        sig.add((entry.name, -1, -1))
            return sig
        except FileNotFoundError:
            return None
        except OSError as e:
            logger.warning("DirectoryWatcher for %s could not scan (%s); retrying.", self._directory, e)
            return None

    def run(self) -> None:
        last = self._signature()
        armed = last is not None
        if armed:
            logger.debug("DirectoryWatcher armed on %s", self._directory)
        while not self.is_stopped():
            self._stop_event.wait(self._poll_interval)
            if self.is_stopped():
                break
            current = self._signature()
            if current is None:
                # Directory absent (not yet mounted, or mid-swap). Drop the arm; we re-arm when it
                # reappears. This is the poll-loop analogue of the Java WatchService re-arm.
                if armed:
                    logger.warning("DirectoryWatcher target %s disappeared; re-arming.", self._directory)
                armed = False
                last = None
                continue
            if not armed:
                # The directory (re)appeared: arm on its current state and fire so a config that
                # landed while we were unarmed is picked up.
                logger.debug("DirectoryWatcher (re-)armed on %s", self._directory)
                armed = True
                last = current
                self.do_on_change()
                continue
            if current != last:
                last = current
                self.do_on_change()
