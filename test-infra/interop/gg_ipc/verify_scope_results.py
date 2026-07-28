#!/usr/bin/env python3
"""Verify one deployed Greengrass IPC scoped-command evidence set (0.5.0).

The deployed ``gg-scope-matrix`` role writes one JSON document per physical actor.  This
verifier turns those documents into a strict cross-language matrix of the four claims the
0.5.0 scoped-command wave makes on the wire:

1. ``describe`` advertises ``scope`` on EVERY manifest entry - the four scope-indifferent
   built-ins as ``both``, the custom INSTANCE verb as ``instance``, the custom COMPONENT
   verb as ``component`` - with exactly the key set ``{verb, builtIn, scope}``.
2. An instance-topic-addressed command carrying NO ``body.instance`` routes, and the
   handler receives the topic's instance token.
3. A topic/body instance conflict is rejected ``BAD_ARGS`` with the byte-pinned message.
4. A COMPONENT-scoped verb refuses an instance-addressed delivery with its own byte-pinned
   message.

Each is asserted for every requester/responder language pair, and the responder's describe
digest must agree across every language that pulled it (a per-language canonicalization
divergence would otherwise be invisible).
"""

import argparse
import json
import sys
from pathlib import Path

SCHEMA = "edgecommons.gg-ipc-scope.v1"

# The exact manifest every ``gg-scope-matrix`` responder must advertise: the four
# scope-indifferent built-ins plus `reload-config`, plus the two custom verbs the role
# registers, sorted by verb.  No entry carries `availability` - none was declared.
EXPECTED_COMMANDS = [
    {"verb": "describe", "builtIn": True, "scope": "both"},
    {"verb": "get-configuration", "builtIn": True, "scope": "both"},
    {"verb": "ping", "builtIn": True, "scope": "both"},
    {"verb": "reload-config", "builtIn": True, "scope": "both"},
    {"verb": "sb/discover", "builtIn": False, "scope": "component"},
    {"verb": "sb/probe", "builtIn": False, "scope": "instance"},
    {"verb": "status", "builtIn": True, "scope": "both"},
]

ADDRESSED_INSTANCE = "kep1"
CONFLICT_MESSAGE = "instance in body conflicts with the addressed instance"
COMPONENT_SCOPE_MESSAGE = "verb 'sb/discover' is component-scoped"
BAD_ARGS = "BAD_ARGS"


def target_actor(requester: str, target_language: str) -> str:
    """Map the Rust self-pair to its separately deployed peer component."""
    if requester == "rust" and target_language == "rust":
        return "rustpeer"
    return target_language


def logical_language(actor: str) -> str:
    return "rust" if actor == "rustpeer" else actor


def load_result(directory: Path, run_id: str, actor: str) -> tuple[dict | None, list[str]]:
    path = directory / f"edgecommons_gg_ipc_scope_{actor}_{run_id}.json"
    if not path.is_file():
        return None, [f"missing evidence file: {path}"]
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        return None, [f"unreadable evidence file {path}: {error}"]
    if not isinstance(data, dict):
        return None, [f"evidence file is not an object: {path}"]
    return data, []


def verify_describe(edge: object, prefix: str) -> tuple[list[str], str | None]:
    """The manifest SHAPE claim: every entry is exactly {verb, builtIn, scope}."""
    errors: list[str] = []
    if not isinstance(edge, dict):
        return [f"{prefix} describe evidence missing"], None
    if edge.get("ok") is not True:
        errors.append(f"{prefix} describe probe did not pass: {edge.get('error')}")
    commands = edge.get("commands")
    if not isinstance(commands, list):
        errors.append(f"{prefix} describe evidence carries no commands[]")
        return errors, None
    if len(commands) != len(EXPECTED_COMMANDS):
        errors.append(
            f"{prefix} describe advertised {len(commands)} verbs, expected"
            f" {len(EXPECTED_COMMANDS)}: {commands}")
    for expected, entry in zip(EXPECTED_COMMANDS, commands):
        if not isinstance(entry, dict):
            errors.append(f"{prefix} describe entry is not an object: {entry}")
            continue
        if set(entry) != set(expected):
            errors.append(
                f"{prefix} the {expected['verb']} entry members must be exactly"
                f" {sorted(expected)}, got {sorted(entry)}")
        if entry != expected:
            errors.append(f"{prefix} {expected['verb']} entry mismatch: {entry}")

    digest = edge.get("digest")
    if not isinstance(digest, str) or not digest.startswith("sha256:") or len(digest) != 71:
        errors.append(f"{prefix} digest must be 'sha256:' + 64 hex chars, got {digest!r}")
        return errors, None
    try:
        int(digest[len("sha256:"):], 16)
    except ValueError:
        errors.append(f"{prefix} digest is not hexadecimal: {digest!r}")
        return errors, None
    return errors, digest


def verify_instance_routing(edge: object, prefix: str, target_language: str) -> list[str]:
    """Instance-topic addressing with no body.instance reaches the handler with the token."""
    if not isinstance(edge, dict):
        return [f"{prefix} instance-routing evidence missing"]
    errors: list[str] = []
    if edge.get("ok") is not True:
        errors.append(f"{prefix} instance-routing probe did not pass: {edge.get('error')}")
    if edge.get("addressed_instance") != ADDRESSED_INSTANCE:
        errors.append(
            f"{prefix} the handler did not receive the addressed instance"
            f" {ADDRESSED_INSTANCE!r}: {edge.get('addressed_instance')!r}")
    body = edge.get("reply_body")
    if not isinstance(body, dict):
        errors.append(f"{prefix} instance-routing reply carried no result object")
        return errors
    if body.get("instance") != ADDRESSED_INSTANCE:
        errors.append(f"{prefix} reply result.instance was {body.get('instance')!r}")
    if body.get("probe") != target_language:
        errors.append(
            f"{prefix} reply came from {body.get('probe')!r}, expected {target_language!r}")
    return errors


def verify_rejection(edge: object, prefix: str, expected_message: str, label: str) -> list[str]:
    """A byte-pinned coded rejection: the code AND the exact message text."""
    if not isinstance(edge, dict):
        return [f"{prefix} {label} evidence missing"]
    errors: list[str] = []
    if edge.get("ok") is not True:
        errors.append(f"{prefix} {label} probe did not pass: {edge.get('error')}")
    if edge.get("code") != BAD_ARGS:
        errors.append(f"{prefix} {label} code was {edge.get('code')!r}, expected {BAD_ARGS!r}")
    if edge.get("message") != expected_message:
        errors.append(
            f"{prefix} {label} message was {edge.get('message')!r}, expected"
            f" {expected_message!r}")
    return errors


def verify_actor(
    result: dict,
    run_id: str,
    actor: str,
    languages: tuple[str, ...],
    digests: dict[str, dict[str, str]],
) -> list[str]:
    errors: list[str] = []
    prefix = f"{actor}:"
    if result.get("schema") != SCHEMA:
        errors.append(f"{prefix} wrong schema: {result.get('schema')!r}")
    if result.get("run_id") != run_id:
        errors.append(f"{prefix} wrong run_id")
    if result.get("actor") != actor:
        errors.append(f"{prefix} wrong actor identity")
    if result.get("language") != logical_language(actor):
        errors.append(f"{prefix} wrong logical language")
    if result.get("ready_missing"):
        errors.append(f"{prefix} readiness barrier incomplete: {result.get('ready_missing')}")
    if result.get("done_missing"):
        errors.append(f"{prefix} completion barrier incomplete: {result.get('done_missing')}")
    if result.get("errors"):
        errors.append(f"{prefix} role reported errors: {result.get('errors')}")
    if result.get("ok") is not True:
        errors.append(f"{prefix} role did not report success")

    canonical = actor != "rustpeer"
    if result.get("canonical_actor") is not canonical:
        errors.append(f"{prefix} canonical_actor flag is incorrect")

    targets = result.get("targets")
    if not canonical:
        if targets:
            errors.append(f"{prefix} rustpeer must not add a duplicate logical matrix row")
        return errors

    if not isinstance(targets, dict) or set(targets) != set(languages):
        errors.append(f"{prefix} scoped-command matrix row is incomplete: {targets}")
        return errors

    for language in languages:
        mapped = target_actor(actor, language)
        edge = targets.get(language)
        pair = f"{actor}->{language}"
        if not isinstance(edge, dict):
            errors.append(f"{pair}: missing target evidence")
            continue
        if edge.get("target_actor") != mapped:
            errors.append(f"{pair}: wrong target actor {edge.get('target_actor')!r}")

        describe_errors, digest = verify_describe(edge.get("describe"), f"{pair}:")
        errors.extend(describe_errors)
        if digest is not None:
            digests.setdefault(mapped, {})[actor] = digest

        errors.extend(verify_instance_routing(
            edge.get("instance_routing"), f"{pair}:", language))
        errors.extend(verify_rejection(
            edge.get("conflict"), f"{pair}:", CONFLICT_MESSAGE, "topic/body conflict"))
        errors.extend(verify_rejection(
            edge.get("component_scope"), f"{pair}:", COMPONENT_SCOPE_MESSAGE,
            "component-scope refusal"))
    return errors


def verify_digest_agreement(digests: dict[str, dict[str, str]]) -> list[str]:
    """One responder's manifest must hash identically in every language that pulled it."""
    errors: list[str] = []
    for responder, observed in sorted(digests.items()):
        distinct = sorted(set(observed.values()))
        if len(distinct) > 1:
            errors.append(
                f"the languages disagree on {responder}'s describe digest: {observed}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", type=Path, required=True,
                        help="directory containing the copied /tmp result JSON")
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--languages", default="python,java,rust,ts",
                        help="csv of the logical languages that were deployed")
    args = parser.parse_args()

    languages = tuple(part for part in args.languages.split(",") if part)
    actors = list(languages)
    if "rust" in languages:
        actors.insert(languages.index("rust") + 1, "rustpeer")

    failures: list[str] = []
    results: dict[str, dict] = {}
    digests: dict[str, dict[str, str]] = {}
    for actor in actors:
        result, load_errors = load_result(args.directory, args.run_id, actor)
        failures.extend(load_errors)
        if result is not None:
            results[actor] = result
            failures.extend(verify_actor(result, args.run_id, actor, languages, digests))
    failures.extend(verify_digest_agreement(digests))

    summary = {
        "schema": "edgecommons.gg-ipc-scope-verification.v1",
        "ok": not failures and len(results) == len(actors),
        "run_id": args.run_id,
        "actors": sorted(results),
        "languages": list(languages),
        "logical_matrix": {
            "describe_scope_edges": len(languages) ** 2,
            "instance_routing_edges": len(languages) ** 2,
            "conflict_rejection_edges": len(languages) ** 2,
            "component_scope_refusal_edges": len(languages) ** 2,
            "rust_self_actor": "rustpeer" if "rust" in languages else None,
        },
        "describe_digests": {
            responder: sorted(set(observed.values()))
            for responder, observed in sorted(digests.items())
        },
        "errors": failures,
    }
    print(json.dumps(summary, indent=2, sort_keys=True))
    return 0 if summary["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
