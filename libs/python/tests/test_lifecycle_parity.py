"""
Unit tests for lifecycle / robustness parity fixes (no broker required):
- MessageTags.to_dict omits "thing" when there is no thing name
- MetricEmitter.shutdown closes the target and resets state
- FileConfigManager.close stops the file-watcher observer (no thread leak)
- the config file watcher reacts to create/move and isolates reload errors
"""

import json
import logging

from ggcommons.messaging.message import MessageTags
from ggcommons.metrics.metric_emitter import MetricEmitter
from ggcommons.config.manager.file_config_manager import (
    FileConfigManager,
    ConfigFileChangeEventHandler,
)


def test_message_tags_omits_null_thing():
    assert "thing" not in MessageTags(None, {"a": "b"}).to_dict()
    assert MessageTags("t", {}).to_dict()["thing"] == "t"


def test_metric_emitter_shutdown_closes_target_and_resets():
    class FakeTarget:
        def __init__(self):
            self.closed = False

        def close(self):
            self.closed = True

    target = FakeTarget()
    MetricEmitter.metric_target = target
    MetricEmitter.metrics = {"m": object()}
    try:
        MetricEmitter.shutdown()
        assert target.closed is True
        assert MetricEmitter.metric_target is None
        assert MetricEmitter.metrics == {}
        # Idempotent: a second call must not raise.
        MetricEmitter.shutdown()
    finally:
        MetricEmitter.metric_target = None
        MetricEmitter.metrics = {}


def _write_config(path):
    path.write_text(json.dumps({"component": {"global": {}, "instances": []}}))


def test_file_config_manager_close_stops_observer(tmp_path):
    cfg = tmp_path / "config.json"
    _write_config(cfg)
    root = logging.getLogger()
    saved = root.handlers[:]
    fcm = FileConfigManager("com.test.C", "thing-1", str(cfg))
    try:
        observer = fcm._observer
        assert observer.is_alive()
        fcm.close()
        assert not observer.is_alive()
        assert fcm._observer is None
        # close() is idempotent: a second call is a no-op.
        fcm.close()
    finally:
        # restore root handlers mutated by logging setup
        for h in root.handlers[:]:
            root.removeHandler(h)
        for h in saved:
            root.addHandler(h)


def test_watcher_handles_create_move_and_isolates_errors():
    reloads = []

    class FakeCM:
        def _load_configuration(self):
            return {"component": {}}

        def configuration_changed(self, cfg):
            reloads.append(cfg)

    handler = ConfigFileChangeEventHandler(FakeCM(), "/some/dir/config.json")

    class Evt:
        def __init__(self, src=None, dest=None, is_dir=False):
            self.src_path = src
            self.dest_path = dest
            self.is_directory = is_dir

    # create + move (atomic save-and-rename) both trigger a reload
    handler.on_created(Evt(src="/some/dir/config.json"))
    handler.on_moved(Evt(src="/tmp/x", dest="/some/dir/config.json"))
    # unrelated file is ignored
    handler.on_modified(Evt(src="/some/dir/other.json"))
    assert len(reloads) == 2

    # a reload error is isolated (does not propagate out of the handler)
    class BoomCM:
        def _load_configuration(self):
            raise RuntimeError("parse error")

        def configuration_changed(self, cfg):
            pass

    boom_handler = ConfigFileChangeEventHandler(BoomCM(), "/some/dir/config.json")
    boom_handler.on_modified(Evt(src="/some/dir/config.json"))  # must not raise
