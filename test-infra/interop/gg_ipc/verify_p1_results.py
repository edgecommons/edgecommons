#!/usr/bin/env python3
"""Verify one deployed Greengrass IPC P1 evidence set.

The deployed role writes one JSON document for each physical actor.  This verifier
turns those five documents into a strict four-language command and confirmed-publish
matrix: the second Rust actor is required only for the Rust-to-Rust edge.
"""

import argparse
import json
import sys
from pathlib import Path


LANGUAGES = ("python", "java", "rust", "ts")
ACTORS = ("python", "java", "rust", "rustpeer", "ts")
SCHEMA = "edgecommons.gg-ipc-p1.v1"
DUPLICATE_WINDOW_MS = 750


def target_actor(requester: str, target_language: str) -> str:
    """Map the Rust self-pair to its separately deployed peer component."""
    if requester == "rust" and target_language == "rust":
        return "rustpeer"
    return target_language


def expected_publishers(actor: str) -> tuple[str, ...]:
    if actor == "rust":
        return tuple(language for language in LANGUAGES if language != "rust")
    if actor == "rustpeer":
        return ("rust",)
    return LANGUAGES


def logical_language(actor: str) -> str:
    """Return the logical language represented by a physical P1 actor."""
    return "rust" if actor == "rustpeer" else actor


def confirmed_topic(run_id: str, publisher: str, actor: str) -> str:
    """Return the exact confirmed-publish topic expected by one receiving actor."""
    return f"edgecommons/interop/p1/{run_id}/confirmed/{publisher}/{actor}"


def load_result(directory: Path, run_id: str, actor: str) -> tuple[dict | None, list[str]]:
    path = directory / f"edgecommons_gg_ipc_p1_{actor}_{run_id}.json"
    if not path.is_file():
        return None, [f"missing evidence file: {path}"]
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return None, [f"unreadable evidence file {path}: {error}"]
    if not isinstance(data, dict):
        return None, [f"evidence file is not an object: {path}"]
    return data, []


def verify_deferred_edge(
    edge: dict,
    run_id: str,
    requester: str,
    target_language: str,
    target_actor_name: str,
) -> list[str]:
    """Validate the raw, post-duplicate-window evidence for one deferred command edge."""
    errors: list[str] = []
    prefix = f"{requester}: deferred request to {target_language}:"
    expected_token = f"{run_id}:{requester}->{target_language}"
    if edge.get("target_actor") != target_actor_name:
        errors.append(f"{prefix} wrong target actor")
    if edge.get("expected_token") != expected_token:
        errors.append(f"{prefix} missing expected request token evidence")
    if edge.get("expected_responder") != target_language:
        errors.append(f"{prefix} missing expected responder evidence")
    if edge.get("expected_responder_actor") != target_actor_name:
        errors.append(f"{prefix} missing expected responder actor evidence")
    if edge.get("reply_count") != 1:
        errors.append(f"{prefix} reply count was not exactly one")
    if edge.get("duplicate_window_ms") != DUPLICATE_WINDOW_MS:
        errors.append(f"{prefix} reply count was not observed through the duplicate window")
    if edge.get("correlation_match") is not True:
        errors.append(f"{prefix} reply correlation did not match the request")

    body = edge.get("reply_body")
    if not isinstance(body, dict) or body.get("ok") is not True:
        errors.append(f"{prefix} reply body is not a successful terminal response")
        return errors
    result = body.get("result")
    if not isinstance(result, dict):
        errors.append(f"{prefix} reply body is missing result")
        return errors
    if result.get("token") != expected_token:
        errors.append(f"{prefix} terminal result token did not match")
    if result.get("responder") != target_language:
        errors.append(f"{prefix} terminal responder did not match")
    if result.get("responderActor") != target_actor_name:
        errors.append(f"{prefix} terminal responder actor did not match")
    if result.get("durablyAccepted") is not True:
        errors.append(f"{prefix} terminal result was not durably accepted")
    return errors


def verify_confirmed_receipt(
    receipt: object,
    run_id: str,
    actor: str,
    publisher: str,
) -> list[str]:
    """Validate the captured topic and body for one strict confirmed-publish receipt."""
    errors: list[str] = []
    prefix = f"{actor}: confirmed receipt from {publisher}:"
    if not isinstance(receipt, dict):
        return [f"{prefix} missing receipt evidence"]
    if receipt.get("count") != 1:
        errors.append(f"{prefix} receipt count was not exactly one")
    if receipt.get("ok") is not True:
        errors.append(f"{prefix} producer did not report a valid receipt")
    items = receipt.get("items")
    if not isinstance(items, list) or len(items) != 1:
        errors.append(f"{prefix} missing exactly one captured receipt item")
        return errors
    item = items[0]
    if not isinstance(item, dict):
        errors.append(f"{prefix} captured receipt item is not an object")
        return errors
    if item.get("ok") is not True:
        errors.append(f"{prefix} captured receipt item was not valid")
    if item.get("topic") != confirmed_topic(run_id, publisher, actor):
        errors.append(f"{prefix} topic did not match the matrix edge")
    body = item.get("body")
    if not isinstance(body, dict):
        errors.append(f"{prefix} body is not an object")
        return errors
    if body.get("runId") != run_id:
        errors.append(f"{prefix} body runId did not match")
    if body.get("publisher") != publisher:
        errors.append(f"{prefix} body publisher did not match")
    if body.get("publisherActor") != publisher:
        errors.append(f"{prefix} body publisher actor did not match")
    if body.get("targetActor") != actor:
        errors.append(f"{prefix} body target actor did not match")
    if body.get("targetLanguage") != logical_language(actor):
        errors.append(f"{prefix} body target language did not match")
    if body.get("strict") is not True:
        errors.append(f"{prefix} body was not marked strict")
    return errors


def verify_actor(result: dict, run_id: str, actor: str) -> list[str]:
    errors: list[str] = []
    prefix = f"{actor}:"
    if result.get("schema") != SCHEMA:
        errors.append(f"{prefix} wrong schema")
    if result.get("run_id") != run_id:
        errors.append(f"{prefix} wrong run_id")
    if result.get("actor") != actor:
        errors.append(f"{prefix} wrong actor identity")
    if result.get("language") != ("rust" if actor == "rustpeer" else actor):
        errors.append(f"{prefix} wrong logical language")
    if result.get("ready_missing"):
        errors.append(f"{prefix} readiness barrier incomplete")
    if result.get("errors"):
        errors.append(f"{prefix} role reported errors")
    if result.get("ok") is not True:
        errors.append(f"{prefix} role did not report success")

    inbound = result.get("confirmed_received")
    if not isinstance(inbound, dict):
        errors.append(f"{prefix} missing confirmed_received evidence")
        inbound = {}
    expected_inbound = expected_publishers(actor)
    if set(inbound) != set(expected_inbound):
        errors.append(f"{prefix} confirmed receipt keys are not the expected matrix row")
    for publisher in expected_inbound:
        errors.extend(
            verify_confirmed_receipt(inbound.get(publisher), run_id, actor, publisher)
        )

    canonical = actor != "rustpeer"
    if result.get("canonical_actor") is not canonical:
        errors.append(f"{prefix} canonical_actor flag is incorrect")
    requests = result.get("deferred_requests")
    publishes = result.get("confirmed_publishes")
    if canonical:
        if not isinstance(requests, dict) or set(requests) != set(LANGUAGES):
            errors.append(f"{prefix} deferred command matrix row is incomplete")
            requests = {}
        if not isinstance(publishes, dict) or set(publishes) != set(LANGUAGES):
            errors.append(f"{prefix} confirmed publish matrix row is incomplete")
            publishes = {}
        for target in LANGUAGES:
            mapped_actor = target_actor(actor, target)
            request = requests.get(target)
            if not isinstance(request, dict) or request.get("ok") is not True:
                errors.append(f"{prefix} deferred request to {target} failed")
            else:
                errors.extend(
                    verify_deferred_edge(request, run_id, actor, target, mapped_actor)
                )
            publish = publishes.get(target)
            if not isinstance(publish, dict) or publish.get("ok") is not True:
                errors.append(f"{prefix} strict publish to {target} failed")
            elif (
                publish.get("target_actor") != mapped_actor
                or publish.get("confirmed") is not True
                or publish.get("qos") != 1
            ):
                errors.append(f"{prefix} strict publish to {target} lacks QoS1 confirmation evidence")
    elif requests or publishes:
        errors.append(f"{prefix} rustpeer must not add a duplicate logical matrix row")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True, help="directory containing copied /tmp result JSON")
    parser.add_argument("--run-id", required=True)
    args = parser.parse_args()

    failures: list[str] = []
    results: dict[str, dict] = {}
    for actor in ACTORS:
        result, load_errors = load_result(args.directory, args.run_id, actor)
        failures.extend(load_errors)
        if result is not None:
            results[actor] = result
            failures.extend(verify_actor(result, args.run_id, actor))
    summary = {
        "schema": "edgecommons.gg-ipc-p1-verification.v1",
        "ok": not failures and len(results) == len(ACTORS),
        "run_id": args.run_id,
        "actors": sorted(results),
        "logical_matrix": {
            "deferred_command_edges": len(LANGUAGES) ** 2,
            "strict_confirmed_publish_edges": len(LANGUAGES) ** 2,
            "rust_self_actor": "rustpeer",
        },
        "errors": failures,
    }
    print(json.dumps(summary, sort_keys=True))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
