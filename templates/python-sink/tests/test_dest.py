"""The sink's invariants, tested without a broker, a transport, or the library.

`app/dest.py` is deliberately library-free: the destination, the error taxonomy and the retry policy
are pure logic, so the properties that make a sink safe -- idempotent redelivery, verification before
release, transient-vs-permanent classification, jittered backoff against a time budget -- are all
testable in-process, in milliseconds. Run them with `pytest`.
"""
import os

import pytest

from app.dest import (
    DEFAULT_MAX_DELAY_MS,
    DEFAULT_MAX_QUEUE,
    DeliverError,
    Delivered,
    Item,
    LocalDestination,
    RetryPolicy,
    build_destination,
    key_for,
    parse_retry,
    parse_sink,
)


def item(key: str, body: str) -> Item:
    return Item(key, body.encode("utf-8"))


# --- the destination -----------------------------------------------------------------------------


def test_delivery_lands_the_object_at_its_stable_key(tmp_path):
    dest = LocalDestination(str(tmp_path))

    it = item("a/b/thing.json", "hello")
    delivered = dest.deliver(it)
    dest.verify(it, delivered)

    assert delivered.bytes_written == 5
    assert (tmp_path / "a" / "b" / "thing.json").read_text() == "hello"


def test_redelivery_overwrites_rather_than_duplicating(tmp_path):
    # This is what makes retry safe. If a redelivery could duplicate, a sink could not retry.
    dest = LocalDestination(str(tmp_path))

    dest.deliver(item("thing.json", "first"))
    second = item("thing.json", "second")
    dest.verify(second, dest.deliver(second))

    assert (tmp_path / "thing.json").read_text() == "second"
    assert len(os.listdir(tmp_path)) == 1, "one object, not two"


def test_no_partial_file_is_left_behind(tmp_path):
    # The write goes to a temp file and is renamed into place: a reader never sees a half-written
    # object, and a crash mid-write leaves no corrupt artifact at the real key.
    dest = LocalDestination(str(tmp_path))
    dest.deliver(item("thing.json", "hello"))

    leftovers = [n for n in os.listdir(tmp_path) if "partial" in n]
    assert leftovers == [], f"the temp file must be renamed, not left: {leftovers}"


def test_verify_refuses_a_mismatch_so_the_source_is_never_released(tmp_path):
    dest = LocalDestination(str(tmp_path))
    it = item("thing.json", "hello")
    dest.deliver(it)

    # Claim we wrote more than we did: verify must catch it.
    with pytest.raises(DeliverError):
        dest.verify(it, Delivered(bytes_written=999))


def test_verify_refuses_an_object_that_is_not_there(tmp_path):
    dest = LocalDestination(str(tmp_path))
    with pytest.raises(DeliverError) as e:
        dest.verify(item("never-written.json", "x"), Delivered(1))
    assert e.value.transient is True


def test_error_classification_decides_whether_retrying_can_help():
    # Retrying a permanent failure burns the budget; giving up on a transient one loses data a second
    # attempt would have delivered.
    assert DeliverError.transient_failure("timeout").transient is True
    assert DeliverError.permanent_failure("bad credentials").transient is False


def test_a_destination_is_built_from_config(tmp_path):
    assert build_destination({"type": "local", "path": str(tmp_path)}).kind() == "local"

    with pytest.raises(ValueError):
        build_destination({"type": "s3", "bucket": "b"})  # not implemented in this scaffold
    with pytest.raises(ValueError):
        build_destination({"type": "local"})  # `path` is required


def test_the_key_is_deterministic():
    # Deterministic is the whole point: the same message must always resolve to the same key, or a
    # retry duplicates instead of overwriting.
    a = key_for("archive", "ecv1/gw/x/main/data/temp", "uuid-1")
    b = key_for("archive", "ecv1/gw/x/main/data/temp", "uuid-1")

    assert a == b
    assert a.startswith("archive/temp/")
    assert key_for("archive", "ecv1/gw/x/main/data/temp", "uuid-2") != a


# --- retry ---------------------------------------------------------------------------------------


def test_backoff_grows_exponentially_and_is_capped():
    r = RetryPolicy(base_delay_ms=1_000, max_delay_ms=10_000, give_up_after_ms=1)

    # With full jitter, rand01 = 1.0 yields the ceiling of the window.
    assert r.delay_ms(0, 1.0) == 1_000
    assert r.delay_ms(1, 1.0) == 2_000
    assert r.delay_ms(2, 1.0) == 4_000
    # ...and it is capped, so a long outage does not back off to next week.
    assert r.delay_ms(20, 1.0) == 10_000
    assert r.delay_ms(999, 1.0) == 10_000


def test_jitter_spreads_the_retries():
    # The point of full jitter: two components that lost the same endpoint do NOT retry in lockstep.
    # The delay is a random point *in* the window, not the window's edge.
    r = RetryPolicy(base_delay_ms=1_000, max_delay_ms=60_000, give_up_after_ms=1)

    assert r.delay_ms(3, 0.0) == 0, "the window's floor is immediate"
    assert r.delay_ms(3, 0.5) == 4_000, "half way into an 8s window"
    assert r.delay_ms(3, 1.0) == 8_000

    # And without an explicit rand01 it really is random: 50 draws from an 8s window must not all
    # land on the same instant.
    draws = {r.delay_ms(3) for _ in range(50)}
    assert len(draws) > 1, "unjittered backoff is a synchronized thundering herd"
    assert all(0 <= d <= 8_000 for d in draws)


def test_the_give_up_is_a_time_budget_not_an_attempt_count():
    r = RetryPolicy(base_delay_ms=1, max_delay_ms=1, give_up_after_ms=5_000)

    assert r.budget_spent(4_999.0) is False
    assert r.budget_spent(5_000.0) is True


def test_a_retry_policy_parses_and_inherits_the_global_defaults():
    policy = parse_retry({"baseDelayMs": 500}, {"giveUpAfterMs": 60_000})

    assert policy.base_delay_ms == 500
    assert policy.give_up_after_ms == 60_000, "inherited from component.global.defaults.retry"
    assert policy.max_delay_ms == DEFAULT_MAX_DELAY_MS, "the unspecified field takes its default"

    with pytest.raises(ValueError, match="unknown retry key"):
        parse_retry({"baseDelayMS": 500})  # a typo is a mistake, not a no-op


# --- sink config ---------------------------------------------------------------------------------


def test_a_sink_parses_from_its_instance_config():
    sink = parse_sink(
        {
            "id": "archive",
            "subscribe": "ecv1/+/+/+/data/#",
            "destination": {"type": "local", "path": "/var/lib/out"},
            "retry": {"baseDelayMs": 500, "giveUpAfterMs": 60_000},
        }
    )

    assert sink.id == "archive"
    assert sink.retry.base_delay_ms == 500
    assert sink.retry.max_delay_ms == DEFAULT_MAX_DELAY_MS
    assert sink.max_queue == DEFAULT_MAX_QUEUE, "the queue is bounded by default"
    assert sink.build_destination().kind() == "local"


def test_global_defaults_apply_to_a_sink_that_does_not_override_them():
    defaults = {"retry": {"baseDelayMs": 250}, "maxQueue": 8}

    sink = parse_sink(
        {"id": "s", "subscribe": "t", "destination": {"type": "local", "path": "/out"}}, defaults
    )

    assert sink.retry.base_delay_ms == 250
    assert sink.max_queue == 8


def test_a_bad_sink_is_rejected_at_config_time_not_on_the_first_message():
    good = {"id": "s", "subscribe": "t", "destination": {"type": "local", "path": "/out"}}

    with pytest.raises(ValueError, match="unknown sink key"):
        parse_sink({**good, "destinaton": {}})
    with pytest.raises(ValueError):
        parse_sink({"id": "s", "subscribe": "t"})  # no destination
    with pytest.raises(ValueError):
        parse_sink({**good, "destination": {"type": "nowhere"}})
    with pytest.raises(ValueError):
        parse_sink({**good, "maxQueue": 0})
