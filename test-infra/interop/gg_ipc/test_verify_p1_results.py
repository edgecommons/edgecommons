"""Focused contract tests for the deployed P1 evidence verifier."""

import copy
import importlib.util
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("verify_p1_results.py")
SPEC = importlib.util.spec_from_file_location("verify_p1_results", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


RUN_ID = "p1-unit"


def _deferred_edge(requester, target_language):
    target_actor = VERIFIER.target_actor(requester, target_language)
    token = f"{RUN_ID}:{requester}->{target_language}"
    return {
        "ok": True,
        "target_actor": target_actor,
        "expected_token": token,
        "expected_responder": target_language,
        "expected_responder_actor": target_actor,
        "reply_count": 1,
        "correlation_match": True,
        "duplicate_window_ms": 750,
        "reply_body": {
            "ok": True,
            "result": {
                "token": token,
                "responder": target_language,
                "responderActor": target_actor,
                "durablyAccepted": True,
            },
        },
    }


def _confirmed_receipt(actor, publisher):
    return {
        "count": 1,
        "ok": True,
        "items": [{
            "ok": True,
            "topic": VERIFIER.confirmed_topic(RUN_ID, publisher, actor),
            "body": {
                "runId": RUN_ID,
                "publisher": publisher,
                "publisherActor": publisher,
                "targetLanguage": VERIFIER.logical_language(actor),
                "targetActor": actor,
                "strict": True,
            },
        }],
    }


def _actor_result(actor):
    canonical = actor != "rustpeer"
    result = {
        "schema": VERIFIER.SCHEMA,
        "ok": True,
        "run_id": RUN_ID,
        "actor": actor,
        "language": "rust" if actor == "rustpeer" else actor,
        "canonical_actor": canonical,
        "ready_missing": [],
        "errors": {},
        "confirmed_received": {
            publisher: _confirmed_receipt(actor, publisher)
            for publisher in VERIFIER.expected_publishers(actor)
        },
        "deferred_requests": {},
        "confirmed_publishes": {},
    }
    if canonical:
        result["deferred_requests"] = {
            target: _deferred_edge(actor, target) for target in VERIFIER.LANGUAGES
        }
        result["confirmed_publishes"] = {
            target: {
                "ok": True,
                "target_actor": VERIFIER.target_actor(actor, target),
                "confirmed": True,
                "qos": 1,
            }
            for target in VERIFIER.LANGUAGES
        }
    return result


def test_verify_actor_accepts_complete_per_edge_evidence():
    for actor in VERIFIER.ACTORS:
        assert VERIFIER.verify_actor(_actor_result(actor), RUN_ID, actor) == []


def test_verify_actor_rejects_uncorrelated_or_duplicate_deferred_response():
    result = _actor_result("python")
    result["deferred_requests"]["java"]["correlation_match"] = False
    result["deferred_requests"]["rust"]["reply_count"] = 2

    errors = VERIFIER.verify_actor(result, RUN_ID, "python")

    assert any("java: reply correlation did not match" in error for error in errors)
    assert any("rust: reply count was not exactly one" in error for error in errors)


def test_verify_actor_rejects_forged_terminal_acceptance_claim():
    result = copy.deepcopy(_actor_result("ts"))
    result["deferred_requests"]["java"]["reply_body"]["result"]["durablyAccepted"] = False
    result["deferred_requests"]["java"]["duplicate_window_ms"] = 0

    errors = VERIFIER.verify_actor(result, RUN_ID, "ts")

    assert any("java: terminal result was not durably accepted" in error for error in errors)
    assert any("java: reply count was not observed through the duplicate window" in error for error in errors)


def test_verify_actor_rejects_confirmed_receipt_without_a_captured_item():
    result = _actor_result("python")
    result["confirmed_received"]["java"]["items"] = []

    errors = VERIFIER.verify_actor(result, RUN_ID, "python")

    assert any("java: missing exactly one captured receipt item" in error for error in errors)


def test_verify_actor_rejects_tampered_confirmed_topic_and_body():
    result = _actor_result("rustpeer")
    item = result["confirmed_received"]["rust"]["items"][0]
    item["topic"] = "edgecommons/interop/p1/wrong/confirmed/rust/rustpeer"
    item["body"].update({
        "runId": "wrong",
        "publisher": "ts",
        "publisherActor": "ts",
        "targetLanguage": "ts",
        "targetActor": "ts",
        "strict": False,
    })

    errors = VERIFIER.verify_actor(result, RUN_ID, "rustpeer")

    assert any("rust: topic did not match the matrix edge" in error for error in errors)
    assert any("rust: body runId did not match" in error for error in errors)
    assert any("rust: body publisher did not match" in error for error in errors)
    assert any("rust: body publisher actor did not match" in error for error in errors)
    assert any("rust: body target language did not match" in error for error in errors)
    assert any("rust: body target actor did not match" in error for error in errors)
    assert any("rust: body was not marked strict" in error for error in errors)
