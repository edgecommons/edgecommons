"""
Unit tests for metric parity fixes (no broker / AWS required):
- M9: 10-dimension cap enforced on the Metric itself
- M2: constructor does not mutate caller-supplied dicts
- H6: MetricEmitter.is_metric_defined is a pure lookup
- H8: CloudWatch flush chunks to <=1000 datums and isolates per-namespace failures
"""

import logging
import threading
import pytest

from edgecommons.metrics.metric import Metric
from edgecommons.metrics.metric_emitter import MetricEmitter


def _metric(name="m", dims=None):
    return Metric("thing", "comp", name, namespace="ns", dimensions=dims)


def test_dimension_cap_enforced_on_metric():
    m = _metric()  # starts with 3 default dims (coreName/category/component)
    for i in range(7):  # 3 + 7 = 10 (the cap)
        m.add_dimension(f"d{i}", str(i))
    assert len(m.get_dimensions()) == 10
    with pytest.raises(ValueError, match="at most 10 dimensions"):
        m.add_dimension("one_too_many", "x")


def test_dimension_overwrite_allowed_at_cap():
    m = _metric()
    for i in range(7):
        m.add_dimension(f"d{i}", str(i))
    # Overwriting an existing key at the cap is allowed (count unchanged).
    m.add_dimension("d0", "updated")
    assert m.get_dimensions()["d0"] == "updated"
    assert len(m.get_dimensions()) == 10


def test_constructor_does_not_mutate_caller_dict():
    caller_dims = {"region": "us-east-1"}
    _metric(dims=caller_dims)
    # The default dimensions must not have leaked back into the caller's dict.
    assert caller_dims == {"region": "us-east-1"}


def test_is_metric_defined_pure_lookup():
    name = "parity_probe_metric"
    assert MetricEmitter.is_metric_defined(name) is False
    try:
        MetricEmitter.define_metric(_metric(name))
        assert MetricEmitter.is_metric_defined(name) is True
        # Pure lookup: calling it does not register anything new.
        before = dict(MetricEmitter.metrics)
        MetricEmitter.is_metric_defined("never_defined")
        assert MetricEmitter.metrics == before
    finally:
        MetricEmitter.metrics.pop(name, None)


def test_cloudwatch_flush_chunks_and_isolates():
    pytest.importorskip("boto3")
    from edgecommons.metrics.targets.cloudwatch import CloudWatch

    class FakeClient:
        def __init__(self, fail_namespaces=()):
            self.calls = []  # (namespace, batch_size)
            self._fail = set(fail_namespaces)

        def put_metric_data(self, Namespace, MetricData):
            if Namespace in self._fail:
                raise RuntimeError("boom")
            self.calls.append((Namespace, len(MetricData)))

    # Build an instance without running __init__ (avoids boto3 client + threads).
    cw = object.__new__(CloudWatch)
    cw.logger = logging.getLogger("test_cw")
    cw._pending_lock = threading.Lock()

    # 2300 datums in nsA -> 1000 + 1000 + 300; nsB small.
    fake = FakeClient()
    cw._cloudwatch_client = fake
    cw._pending_metrics = {"nsA": list(range(2300)), "nsB": list(range(5))}
    cw._flush_metrics()

    assert ("nsA", 1000) in fake.calls
    assert fake.calls.count(("nsA", 1000)) == 2
    assert ("nsA", 300) in fake.calls
    assert ("nsB", 5) in fake.calls
    # Everything sent -> pending cleared.
    assert cw._pending_metrics == {"nsA": [], "nsB": []}

    # Failure isolation: nsA fails, nsB still sends; nsA retained for retry.
    failing = FakeClient(fail_namespaces={"nsA"})
    cw._cloudwatch_client = failing
    cw._pending_metrics = {"nsA": list(range(10)), "nsB": list(range(3))}
    cw._flush_metrics()
    assert ("nsB", 3) in failing.calls
    assert cw._pending_metrics["nsA"] == list(range(10))  # retained
    assert cw._pending_metrics["nsB"] == []
