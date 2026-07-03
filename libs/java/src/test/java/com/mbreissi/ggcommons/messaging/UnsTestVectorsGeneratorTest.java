/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import com.google.gson.JsonArray;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileAlreadyExistsException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;

/**
 * Generates (first run) and then verifies the cross-language UNS conformance vectors under
 * {@code uns-test-vectors/} at the repo root (UNS-CANONICAL-DESIGN §7, D-U12: Java is the
 * canonical generator). The Python/Rust/TS ports load these same files and must build
 * byte-identical topics/filters, fail with the identical error codes, agree on the reserved-class
 * guard verdicts, and reproduce the golden envelopes structurally (D-U22).
 *
 * <p>Gated exactly like the vault generator ({@code cross_language_test_vectors} in
 * {@code libs/rust}): the files are written only when absent (a concurrent run losing the
 * {@code CREATE_NEW} race simply falls through), then ALWAYS verified in place — the committed
 * bytes must equal a fresh recomputation (line-ending-normalized, since {@code core.autocrlf}
 * may rewrite the working tree on Windows). Every case is also replayed through the live
 * implementation ({@link UnsTestVectors}) before writing, so the files can never pin behavior
 * the implementation does not have.
 *
 * <p>Regenerate by deleting {@code uns-test-vectors/} and re-running this test.
 */
class UnsTestVectorsGeneratorTest {

    /** One pinned timestamp for every golden envelope (deterministic, no randomness). */
    private static final String TIMESTAMP = "2026-07-01T12:00:00Z";

    private static final String[] SINGLE_LEVELS = {"device"};
    private static final String[] SINGLE_VALUES = {"gw-01"};
    private static final String[] MULTI_LEVELS = {"site", "zone", "device"};
    private static final String[] MULTI_VALUES = {"dallas", "zone-3", "gw-01"};

    /** The §1.1 design-doc identity (4 levels) — used by the two {@code state} golden envelopes. */
    private static final MessageIdentity DESIGN_IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("factory", "finishing"),
                    new MessageIdentity.HierEntry("zone", "zone-3"),
                    new MessageIdentity.HierEntry("device", "gw-01")),
            "opcua-adapter", "main");

    /** The zero-config single-level identity — used by the remaining golden envelopes. */
    private static final MessageIdentity SINGLE_IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")),
            "opcua-adapter", "main");

    @Test
    void generateAndVerifyCrossLanguageVectors() throws Exception {
        JsonObject topics = topicsDocument();
        JsonObject envelopes = envelopesDocument();
        JsonObject bcast = bcastDocument();

        // Self-check BEFORE writing: the documents must be exactly what the implementation does.
        UnsTestVectors.assertTopicsDocument(topics);
        UnsTestVectors.assertEnvelopesDocument(envelopes);
        UnsTestVectors.assertBcastDocument(bcast);

        String topicsJson = UnsTestVectors.GSON.toJson(topics) + "\n";
        String envelopesJson = UnsTestVectors.GSON.toJson(envelopes) + "\n";
        String bcastJson = UnsTestVectors.GSON.toJson(bcast) + "\n";

        Files.createDirectories(UnsTestVectors.DIR);
        Path topicsPath = UnsTestVectors.DIR.resolve("topics.json");
        Path envelopesPath = UnsTestVectors.DIR.resolve("envelopes.json");
        Path bcastPath = UnsTestVectors.DIR.resolve("bcast.json");
        Path readmePath = UnsTestVectors.DIR.resolve("README.md");
        writeIfAbsent(topicsPath, topicsJson);
        writeIfAbsent(envelopesPath, envelopesJson);
        writeIfAbsent(bcastPath, bcastJson);
        writeIfAbsent(readmePath, README);

        // Determinism lock (verify-in-place): the on-disk vectors must equal the reference
        // computation - re-running is a clean verify, never a rewrite.
        verifyInPlace(topicsPath, topicsJson);
        verifyInPlace(envelopesPath, envelopesJson);
        verifyInPlace(bcastPath, bcastJson);
        verifyInPlace(readmePath, README);
    }

    // ===================== topics.json =====================

    private static JsonObject topicsDocument() {
        JsonObject doc = new JsonObject();
        doc.addProperty("description", "ggcommons UNS cross-language conformance vectors - "
                + "build/validate/filter/guard (UNS-CANONICAL-DESIGN 2.2/4.1; single-fault cases,"
                + " D-U26)");
        doc.add("build", buildCases());
        doc.add("validate", validateCases());
        doc.add("filter", filterCases());
        doc.add("guard", guardCases());
        return doc;
    }

    private static JsonArray buildCases() {
        JsonArray cases = new JsonArray();

        // --- happy paths: every class, leaf vs channeled ---
        cases.add(buildCase("build-state-leaf", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw-01/opcua-adapter/main/state")));
        cases.add(buildCase("build-cfg-leaf", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "cfg", null,
                topic("ecv1/gw-01/opcua-adapter/main/cfg")));
        cases.add(buildCase("build-metric-channel", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "metric", "sys",
                topic("ecv1/gw-01/opcua-adapter/main/metric/sys")));
        cases.add(buildCase("build-log-channel", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "log", "tail",
                topic("ecv1/gw-01/opcua-adapter/main/log/tail")));
        cases.add(buildCase("build-data-channel", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "temp",
                topic("ecv1/gw-01/opcua-adapter/main/data/temp")));
        cases.add(buildCase("build-evt-channel", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "evt", "door-open",
                topic("ecv1/gw-01/opcua-adapter/main/evt/door-open")));
        cases.add(buildCase("build-cmd-namespaced-channel", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "cmd", "sb/status",
                topic("ecv1/gw-01/opcua-adapter/main/cmd/sb/status")));
        cases.add(buildCase("build-app-channel-named-like-a-class", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "app", "state",
                topic("ecv1/gw-01/opcua-adapter/main/app/state")));
        cases.add(buildCase("build-instance-token", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "kep1", false, "data", "temp",
                topic("ecv1/gw-01/opcua-adapter/kep1/data/temp")));
        cases.add(buildCase("build-dots-are-legal", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "v1.2",
                topic("ecv1/gw-01/opcua-adapter/main/data/v1.2")));

        // --- includeRoot true/false, incl. the D-U25 single-level no-op ---
        cases.add(buildCase("build-include-root-multi-level-leaf", MULTI_LEVELS, MULTI_VALUES,
                "opcua-adapter", "main", true, "state", null,
                topic("ecv1/dallas/gw-01/opcua-adapter/main/state")));
        cases.add(buildCase("build-include-root-multi-level-channel", MULTI_LEVELS, MULTI_VALUES,
                "opcua-adapter", "main", true, "data", "temp",
                topic("ecv1/dallas/gw-01/opcua-adapter/main/data/temp")));
        cases.add(buildCase("build-rootless-multi-level-uses-last-hier-value",
                MULTI_LEVELS, MULTI_VALUES,
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw-01/opcua-adapter/main/state")));
        cases.add(buildCase("build-include-root-single-level-noop", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", true, "state", null,
                topic("ecv1/gw-01/opcua-adapter/main/state")));
        cases.add(buildCase("build-include-root-single-level-noop-restores-channel-budget",
                SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", true, "data", "a/b/c",
                topic("ecv1/gw-01/opcua-adapter/main/data/a/b/c")));

        // --- sanitizer <-> validator reconciliation (D-U26: sanitized => valid) ---
        cases.add(buildCase("build-identity-value-with-space", SINGLE_LEVELS,
                new String[]{"gw 01"},
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw 01/opcua-adapter/main/state")));
        cases.add(buildCase("build-identity-value-plus-sanitized", SINGLE_LEVELS,
                new String[]{"gw+01"},
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw_01/opcua-adapter/main/state")));
        cases.add(buildCase("build-identity-value-slash-sanitized", MULTI_LEVELS,
                new String[]{"dal/las", "zone-3", "gw-01"},
                "opcua-adapter", "main", true, "state", null,
                topic("ecv1/dal_las/gw-01/opcua-adapter/main/state")));
        cases.add(buildCase("build-identity-value-c1-control-sanitized", SINGLE_LEVELS,
                new String[]{"gw" + (char) 0x85 + "01"},
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw_01/opcua-adapter/main/state")));
        cases.add(buildCase("build-identity-value-traversal-sanitized", SINGLE_LEVELS,
                new String[]{"gw..01"},
                "opcua-adapter", "main", false, "state", null,
                topic("ecv1/gw_01/opcua-adapter/main/state")));
        cases.add(buildCase("build-component-sanitized", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua+adapter", "main", false, "state", null,
                topic("ecv1/gw-01/opcua_adapter/main/state")));

        // --- hand-built channel/instance tokens are NOT sanitized: the BAD_CHAR path ---
        cases.add(buildCase("build-channel-empty-token", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a//b",
                error("EMPTY_TOKEN")));
        cases.add(buildCase("build-channel-bad-char-plus", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "te+mp",
                error("BAD_CHAR")));
        cases.add(buildCase("build-channel-bad-char-backslash", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a\\b",
                error("BAD_CHAR")));
        cases.add(buildCase("build-channel-bad-char-c0-control", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a" + (char) 0x01 + "b",
                error("BAD_CHAR")));
        cases.add(buildCase("build-channel-bad-char-del", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a" + (char) 0x7F + "b",
                error("BAD_CHAR")));
        cases.add(buildCase("build-channel-bad-char-c1-control", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a" + (char) 0x85 + "b",
                error("BAD_CHAR")));
        cases.add(buildCase("build-channel-traversal", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a..b",
                error("TRAVERSAL")));
        cases.add(buildCase("build-instance-bad-char", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "in+st", false, "data", "temp",
                error("BAD_CHAR")));

        // --- depth boundary: exactly 7 separators ok, 8 rejected (rootless AND rooted) ---
        cases.add(buildCase("build-depth-boundary-rootless-ok", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a/b/c",
                topic("ecv1/gw-01/opcua-adapter/main/data/a/b/c")));
        cases.add(buildCase("build-depth-exceeded-rootless", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "a/b/c/d",
                error("DEPTH_EXCEEDED")));
        cases.add(buildCase("build-depth-boundary-rooted-ok", MULTI_LEVELS, MULTI_VALUES,
                "opcua-adapter", "main", true, "data", "a/b",
                topic("ecv1/dallas/gw-01/opcua-adapter/main/data/a/b")));
        cases.add(buildCase("build-depth-exceeded-rooted", MULTI_LEVELS, MULTI_VALUES,
                "opcua-adapter", "main", true, "data", "a/b/c",
                error("DEPTH_EXCEEDED")));

        // --- length boundary: exactly 256 UTF-8 bytes ok, 257 rejected ---
        // Fixed prefix "ecv1/gw-01/opcua-adapter/main/data/" = 35 ASCII chars.
        cases.add(buildCase("build-length-boundary-ok", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "x".repeat(221),
                topic("ecv1/gw-01/opcua-adapter/main/data/" + "x".repeat(221))));
        cases.add(buildCase("build-length-exceeded", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "x".repeat(222),
                error("LENGTH_EXCEEDED")));

        // --- leaf/channel class rules; an empty channel string means "absent" ---
        cases.add(buildCase("build-channel-on-leaf-state", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "state", "x",
                error("CHANNEL_ON_LEAF")));
        cases.add(buildCase("build-channel-on-leaf-cfg", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "cfg", "a/b",
                error("CHANNEL_ON_LEAF")));
        cases.add(buildCase("build-channel-required-data", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", null,
                error("CHANNEL_REQUIRED")));
        cases.add(buildCase("build-empty-channel-means-absent", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "data", "",
                error("CHANNEL_REQUIRED")));
        cases.add(buildCase("build-channel-required-metric", SINGLE_LEVELS, SINGLE_VALUES,
                "opcua-adapter", "main", false, "metric", null,
                error("CHANNEL_REQUIRED")));

        return cases;
    }

    private static JsonArray validateCases() {
        JsonArray cases = new JsonArray();

        // --- accepting paths ---
        cases.add(validateCase("validate-ok-leaf",
                "ecv1/gw-01/opcua-adapter/main/state", false, ok()));
        cases.add(validateCase("validate-ok-channeled",
                "ecv1/gw-01/opcua-adapter/main/data/temp", false, ok()));
        cases.add(validateCase("validate-ok-multi-token-channel",
                "ecv1/gw-01/opcua-adapter/main/cmd/sb/status", false, ok()));
        cases.add(validateCase("validate-ok-rooted",
                "ecv1/dallas/gw-01/opcua-adapter/main/state", true, ok()));
        cases.add(validateCase("validate-ok-space-token",
                "ecv1/gw 01/opcua-adapter/main/state", false, ok()));
        cases.add(validateCase("validate-ok-depth-boundary",
                "ecv1/d/c/i/data/a/b/c", false, ok()));
        cases.add(validateCase("validate-ok-length-boundary",
                "ecv1/" + "d".repeat(235) + "/comp/main/state", false, ok()));

        // --- the ten error codes, single-fault each ---
        cases.add(validateCase("validate-empty-topic", "", false, error("EMPTY_TOKEN")));
        cases.add(validateCase("validate-empty-token",
                "ecv1//opcua-adapter/main/state", false, error("EMPTY_TOKEN")));
        cases.add(validateCase("validate-bad-char-backslash",
                "ecv1/gw\\01/opcua-adapter/main/state", false, error("BAD_CHAR")));
        cases.add(validateCase("validate-bad-char-c0-control",
                "ecv1/gw" + (char) 0x01 + "01/opcua-adapter/main/state", false,
                error("BAD_CHAR")));
        cases.add(validateCase("validate-bad-char-del",
                "ecv1/gw" + (char) 0x7F + "01/opcua-adapter/main/state", false,
                error("BAD_CHAR")));
        cases.add(validateCase("validate-bad-char-c1-control",
                "ecv1/gw" + (char) 0x85 + "01/opcua-adapter/main/state", false,
                error("BAD_CHAR")));
        cases.add(validateCase("validate-traversal",
                "ecv1/gw-01/a..b/main/state", false, error("TRAVERSAL")));
        cases.add(validateCase("validate-depth-exceeded",
                "ecv1/d/c/i/data/a/b/c/d", false, error("DEPTH_EXCEEDED")));
        cases.add(validateCase("validate-length-exceeded",
                "ecv1/" + "d".repeat(236) + "/comp/main/state", false,
                error("LENGTH_EXCEEDED")));
        cases.add(validateCase("validate-channel-on-leaf",
                "ecv1/gw-01/opcua-adapter/main/state/extra", false, error("CHANNEL_ON_LEAF")));
        cases.add(validateCase("validate-channel-required",
                "ecv1/gw-01/opcua-adapter/main/data", false, error("CHANNEL_REQUIRED")));
        cases.add(validateCase("validate-bad-root",
                "notroot/gw-01/opcua-adapter/main/state", false, error("BAD_ROOT")));
        cases.add(validateCase("validate-bad-root-reply-prefix",
                "ggcommons/reply-42/x/main/state", false, error("BAD_ROOT")));
        cases.add(validateCase("validate-bad-class",
                "ecv1/gw-01/opcua-adapter/main/bogus/x", false, error("BAD_CLASS")));
        cases.add(validateCase("validate-bad-class-uppercase",
                "ecv1/gw-01/opcua-adapter/main/STATE", false, error("BAD_CLASS")));
        cases.add(validateCase("validate-too-short",
                "ecv1/gw-01/opcua-adapter/main", false, error("BAD_CLASS")));
        cases.add(validateCase("validate-wildcard-plus",
                "ecv1/+/opcua-adapter/main/state", false, error("WILDCARD_IN_TOPIC")));
        cases.add(validateCase("validate-wildcard-hash",
                "ecv1/gw-01/opcua-adapter/main/data/#", false, error("WILDCARD_IN_TOPIC")));

        // --- includeRoot sensitivity: the same topic under the two root modes ---
        cases.add(validateCase("validate-rootless-topic-under-rooted-mode",
                "ecv1/gw-01/opcua-adapter/main/state", true, error("BAD_CLASS")));
        cases.add(validateCase("validate-rooted-topic-under-rootless-mode",
                "ecv1/dallas/gw-01/opcua-adapter/main", false, error("BAD_CLASS")));

        return cases;
    }

    private static JsonArray filterCases() {
        JsonArray cases = new JsonArray();
        cases.add(filterCase("filter-all-data", "data", null, null, null, null, false,
                "ecv1/+/+/+/data/#"));
        cases.add(filterCase("filter-all-state-leaf", "state", null, null, null, null, false,
                "ecv1/+/+/+/state"));
        cases.add(filterCase("filter-all-cfg-leaf", "cfg", null, null, null, null, false,
                "ecv1/+/+/+/cfg"));
        cases.add(filterCase("filter-all-metric", "metric", null, null, null, null, false,
                "ecv1/+/+/+/metric/#"));
        cases.add(filterCase("filter-all-log", "log", null, null, null, null, false,
                "ecv1/+/+/+/log/#"));
        cases.add(filterCase("filter-all-evt", "evt", null, null, null, null, false,
                "ecv1/+/+/+/evt/#"));
        cases.add(filterCase("filter-all-app", "app", null, null, null, null, false,
                "ecv1/+/+/+/app/#"));
        cases.add(filterCase("filter-device-pinned", "data", null, "gw-01", null, null, false,
                "ecv1/gw-01/+/+/data/#"));
        cases.add(filterCase("filter-component-pinned", "evt", null, "gw-01", "opcua-adapter",
                null, false, "ecv1/gw-01/opcua-adapter/+/evt/#"));
        cases.add(filterCase("filter-instance-pinned", "cmd", null, "gw-01", "opcua-adapter",
                "kep1", false, "ecv1/gw-01/opcua-adapter/kep1/cmd/#"));
        cases.add(filterCase("filter-rooted-all", "data", null, null, null, null, true,
                "ecv1/+/+/+/+/data/#"));
        cases.add(filterCase("filter-rooted-leaf", "state", null, null, null, null, true,
                "ecv1/+/+/+/+/state"));
        cases.add(filterCase("filter-rooted-site-pinned", "data", "dallas", "gw-01", null, null,
                true, "ecv1/dallas/gw-01/+/+/data/#"));
        cases.add(filterCase("filter-rootless-ignores-site", "data", "dallas", null, null, null,
                false, "ecv1/+/+/+/data/#"));
        return cases;
    }

    private static JsonArray guardCases() {
        JsonArray cases = new JsonArray();
        // Reserved classes at position 4 (always checked).
        cases.add(guardCase("guard-state-reserved", "ecv1/gw-01/comp/main/state", false, true));
        cases.add(guardCase("guard-metric-reserved", "ecv1/gw-01/comp/main/metric/cpu", false, true));
        cases.add(guardCase("guard-cfg-reserved", "ecv1/gw-01/comp/main/cfg", false, true));
        cases.add(guardCase("guard-log-reserved", "ecv1/gw-01/comp/main/log/tail", false, true));
        cases.add(guardCase("guard-position4-checked-even-when-rooted",
                "ecv1/gw-01/comp/main/cfg", true, true));
        // Non-reserved classes pass.
        cases.add(guardCase("guard-data-allowed", "ecv1/gw-01/comp/main/data/temp", false, false));
        cases.add(guardCase("guard-evt-allowed", "ecv1/gw-01/comp/main/evt/x", false, false));
        cases.add(guardCase("guard-cmd-allowed", "ecv1/gw-01/comp/main/cmd/set-config", false, false));
        cases.add(guardCase("guard-app-allowed", "ecv1/gw-01/comp/main/app/anything", false, false));
        // D-U24: position 5 is checked ONLY under includeRoot - unconditional checking would
        // false-positive on legitimate app channels named like a reserved class.
        cases.add(guardCase("guard-app-state-channel-allowed-rootless",
                "ecv1/gw-01/comp/main/app/state", false, false));
        cases.add(guardCase("guard-position5-checked-when-rooted",
                "ecv1/dallas/gw-01/comp/main/state", true, true));
        cases.add(guardCase("guard-position5-unchecked-when-rootless",
                "ecv1/dallas/gw-01/comp/main/state", false, false));
        cases.add(guardCase("guard-rooted-metric-reserved",
                "ecv1/dallas/gw-01/comp/main/metric/cpu", true, true));
        cases.add(guardCase("guard-rooted-app-state-channel-allowed",
                "ecv1/dallas/gw-01/comp/main/app/state", true, false));
        // Non-ecv1 topics are structurally exempt (D-U6/D-U21).
        cases.add(guardCase("guard-non-uns-reply-passes", "ggcommons/reply-8400f2", false, false));
        cases.add(guardCase("guard-cloudwatch-passes", "cloudwatch/metric/put", false, false));
        cases.add(guardCase("guard-root-prefix-but-different-token",
                "ecv1x/gw-01/comp/main/state", false, false));
        cases.add(guardCase("guard-short-topic-passes", "ecv1/gw-01/state", false, false));
        return cases;
    }

    // ===================== envelopes.json =====================

    private static JsonObject envelopesDocument() {
        JsonObject doc = new JsonObject();
        doc.addProperty("description", "ggcommons UNS golden canonical envelopes - one full"
                + " canonical JSON envelope per UNS class, pinned uuid/correlation_id/timestamp"
                + " (D-U13); compare STRUCTURALLY, member order is not normative (D-U22)");
        JsonArray envelopes = new JsonArray();
        envelopes.add(envelopeCase("state-running", "state", null,
                "ecv1/gw-01/opcua-adapter/main/state",
                "state", "1.0", uuid(1, 1), uuid(1, 2), DESIGN_IDENTITY,
                body("{\"status\":\"RUNNING\",\"uptimeSecs\":42}")));
        envelopes.add(envelopeCase("state-stopped", "state", null,
                "ecv1/gw-01/opcua-adapter/main/state",
                "state", "1.0", uuid(2, 1), uuid(2, 2), DESIGN_IDENTITY,
                body("{\"status\":\"STOPPED\"}")));
        envelopes.add(envelopeCase("metric-sys", "metric", "sys",
                "ecv1/gw-01/opcua-adapter/main/metric/sys",
                "Metric", "1.0", uuid(3, 1), uuid(3, 2), SINGLE_IDENTITY,
                body("{\"name\":\"sys\",\"values\":{\"cpu\":12.5,\"memoryUsedPct\":31.4}}")));
        envelopes.add(envelopeCase("cfg-effective-config", "cfg", null,
                "ecv1/gw-01/opcua-adapter/main/cfg",
                "cfg", "1.0", uuid(4, 1), uuid(4, 2), SINGLE_IDENTITY,
                body("{\"config\":{\"component\":{\"name\":\"opcua-adapter\"},"
                        + "\"messaging\":{\"local\":{\"credentials\":\"***\"}}}}")));
        envelopes.add(envelopeCase("log-tail", "log", "tail",
                "ecv1/gw-01/opcua-adapter/main/log/tail",
                "log", "1.0", uuid(5, 1), uuid(5, 2), SINGLE_IDENTITY,
                body("{\"level\":\"INFO\",\"logger\":\"com.example.App\","
                        + "\"message\":\"component started\"}")));
        envelopes.add(envelopeCase("data-signal", "data", "temp",
                "ecv1/gw-01/opcua-adapter/kep1/data/temp",
                "data", "1.0", uuid(6, 1), uuid(6, 2), SINGLE_IDENTITY.withInstance("kep1"),
                body("{\"signalId\":\"ns=2;s=Line1.Temp\",\"value\":21.5,\"quality\":\"GOOD\"}")));
        envelopes.add(envelopeCase("evt-door-open", "evt", "door-open",
                "ecv1/gw-01/opcua-adapter/main/evt/door-open",
                "evt", "1.0", uuid(7, 1), uuid(7, 2), SINGLE_IDENTITY,
                body("{\"event\":\"door-open\",\"severity\":\"info\"}")));
        envelopes.add(envelopeCase("cmd-set-log-level", "cmd", "set-log-level",
                "ecv1/gw-01/opcua-adapter/main/cmd/set-log-level",
                "cmd", "1.0", uuid(8, 1), uuid(8, 2), SINGLE_IDENTITY,
                body("{\"verb\":\"set-log-level\",\"args\":{\"level\":\"DEBUG\"}}")));
        envelopes.add(envelopeCase("app-hello", "app", "hello",
                "ecv1/gw-01/opcua-adapter/main/app/hello",
                "app", "1.0", uuid(9, 1), uuid(9, 2), SINGLE_IDENTITY,
                body("{\"greeting\":\"hello\",\"count\":3}")));
        doc.add("envelopes", envelopes);
        return doc;
    }

    /**
     * Builds one golden-envelope case: the envelope itself is produced by the REAL
     * {@link MessageBuilder} stamping path with the pinned header fields, so the file is
     * Java-implementation output by construction (D-U12).
     */
    private static JsonObject envelopeCase(String name, String cls, String channel, String topic,
            String headerName, String headerVersion, String uuid, String correlationId,
            MessageIdentity identity, JsonObject body) {
        Message message = MessageBuilder.create(headerName, headerVersion)
                .withUuid(uuid)
                .withTimestamp(TIMESTAMP)
                .withCorrelationId(correlationId)
                .withIdentity(identity)
                .withPayload(body)
                .build();
        JsonObject c = new JsonObject();
        c.addProperty("name", name);
        c.addProperty("class", cls);
        if (channel != null) {
            c.addProperty("channel", channel);
        }
        c.addProperty("topic", topic);
        c.add("envelope", message.toDict());
        return c;
    }

    // ===================== bcast.json =====================

    /**
     * The {@code _bcast} republish (reconnect-rehydration) contract — DESIGN-uns §9.3 layer 2 /
     * §9.4, DESIGN-uns-bridge §2.5: the two per-device broadcast command topics, the golden
     * notification envelopes the {@code uns-bridge} publishes (no identity/tags/reply_to, empty
     * body), and the normative listener behavior constants (jitter window / coalescing cooldown)
     * every language's republish listener must implement.
     */
    private static JsonObject bcastDocument() {
        JsonObject doc = new JsonObject();
        doc.addProperty("description", "ggcommons UNS _bcast republish (reconnect-rehydration)"
                + " vectors - the late-join lever (DESIGN-uns 9.3 layer 2 / 9.4,"
                + " DESIGN-uns-bridge 2.5). Topics byte-for-byte; envelopes structural (D-U22);"
                + " the behavior constants are normative for every language's republish listener");
        doc.addProperty("device", "gw-01");
        JsonArray commands = new JsonArray();
        commands.add(bcastCommand("republish-state", "state",
                "00000000-0000-4000-8000-00000000b101", "00000000-0000-4000-8000-00000000b102"));
        commands.add(bcastCommand("republish-cfg", "cfg",
                "00000000-0000-4000-8000-00000000b201", "00000000-0000-4000-8000-00000000b202"));
        doc.add("commands", commands);
        JsonObject behavior = new JsonObject();
        behavior.addProperty("jitterWindowMs", com.mbreissi.ggcommons.uns.RepublishListener.JITTER_WINDOW_MS);
        behavior.addProperty("cooldownMs", com.mbreissi.ggcommons.uns.RepublishListener.COOLDOWN_MS);
        behavior.addProperty("replyTo", false);
        doc.add("behavior", behavior);
        return doc;
    }

    /**
     * One republish command vector: the topic is produced by the REAL topic builder with the
     * reserved {@code _bcast} pseudo-component identity, and the envelope by the REAL
     * {@link MessageBuilder} with no identity (the bridge builds it without a config-bound
     * builder), so the file is Java-implementation output by construction (D-U12).
     */
    private static JsonObject bcastCommand(String verb, String republishes,
            String uuid, String correlationId) {
        MessageIdentity bcast = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-01")), "_bcast", "main");
        String topic = new com.mbreissi.ggcommons.uns.Uns(bcast, false)
                .topic(com.mbreissi.ggcommons.uns.UnsClass.CMD, verb);
        Message message = MessageBuilder.create(verb, "1.0")
                .withUuid(uuid)
                .withTimestamp(TIMESTAMP)
                .withCorrelationId(correlationId)
                .withPayload(new JsonObject())
                .build();
        JsonObject input = new JsonObject();
        input.addProperty("device", "gw-01");
        input.addProperty("component", "_bcast");
        input.addProperty("instance", "main");
        input.addProperty("includeRoot", false);
        input.addProperty("class", "cmd");
        input.addProperty("channel", verb);
        JsonObject c = new JsonObject();
        c.addProperty("name", verb);
        c.addProperty("republishes", republishes);
        c.addProperty("topic", topic);
        c.add("input", input);
        c.add("envelope", message.toDict());
        return c;
    }

    // ===================== case builders =====================

    private static JsonObject buildCase(String name, String[] levels, String[] values,
            String component, String instance, boolean includeRoot, String cls, String channel,
            JsonObject expected) {
        assertEquals(levels.length, values.length, "case '" + name + "': levels/values length");
        JsonObject input = new JsonObject();
        JsonArray levelsArray = new JsonArray();
        JsonObject valuesObject = new JsonObject();
        for (int i = 0; i < levels.length; i++) {
            levelsArray.add(levels[i]);
            valuesObject.addProperty(levels[i], values[i]);
        }
        input.add("hierarchyLevels", levelsArray);
        input.add("identityValues", valuesObject);
        input.addProperty("component", component);
        input.addProperty("instance", instance);
        input.addProperty("includeRoot", includeRoot);
        input.addProperty("class", cls);
        if (channel != null) {
            input.addProperty("channel", channel);
        }
        return vectorCase(name, input, expected);
    }

    private static JsonObject validateCase(String name, String topic, boolean includeRoot,
            JsonObject expected) {
        JsonObject input = new JsonObject();
        input.addProperty("topic", topic);
        input.addProperty("includeRoot", includeRoot);
        return vectorCase(name, input, expected);
    }

    private static JsonObject filterCase(String name, String cls, String site, String device,
            String component, String instance, boolean includeRoot, String expectedFilter) {
        JsonObject scope = new JsonObject();
        if (site != null) {
            scope.addProperty("site", site);
        }
        if (device != null) {
            scope.addProperty("device", device);
        }
        if (component != null) {
            scope.addProperty("component", component);
        }
        if (instance != null) {
            scope.addProperty("instance", instance);
        }
        JsonObject input = new JsonObject();
        input.addProperty("class", cls);
        input.add("scope", scope);
        input.addProperty("includeRoot", includeRoot);
        JsonObject expected = new JsonObject();
        expected.addProperty("filter", expectedFilter);
        return vectorCase(name, input, expected);
    }

    private static JsonObject guardCase(String name, String topic, boolean includeRoot,
            boolean reserved) {
        JsonObject input = new JsonObject();
        input.addProperty("topic", topic);
        input.addProperty("includeRoot", includeRoot);
        JsonObject expected = new JsonObject();
        expected.addProperty("reserved", reserved);
        return vectorCase(name, input, expected);
    }

    private static JsonObject vectorCase(String name, JsonObject input, JsonObject expected) {
        JsonObject c = new JsonObject();
        c.addProperty("name", name);
        c.add("input", input);
        c.add("expected", expected);
        return c;
    }

    private static JsonObject topic(String topic) {
        JsonObject expected = new JsonObject();
        expected.addProperty("topic", topic);
        return expected;
    }

    private static JsonObject error(String code) {
        JsonObject expected = new JsonObject();
        expected.addProperty("error", code);
        return expected;
    }

    private static JsonObject ok() {
        JsonObject expected = new JsonObject();
        expected.addProperty("ok", true);
        return expected;
    }

    private static JsonObject body(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    /** Deterministic pinned UUIDs: vector {@code n}, field {@code f} (1 = uuid, 2 = correlation). */
    private static String uuid(int n, int f) {
        return String.format("00000000-0000-4000-8000-000000000%d0%d", n, f);
    }

    // ===================== write-if-absent + verify-in-place =====================

    /**
     * Writes the file only when absent. {@code CREATE_NEW} is the cross-run race guard (like the
     * vault generator): a concurrent generator losing the race falls through to verify-in-place
     * against the winner's (identical, deterministic) bytes.
     */
    private static void writeIfAbsent(Path path, String content) throws IOException {
        try {
            Files.writeString(path, content, StandardCharsets.UTF_8, StandardOpenOption.CREATE_NEW);
        } catch (FileAlreadyExistsException raced) {
            // Another run (or the committed file) already owns it - verify-in-place follows.
        }
    }

    /**
     * The determinism lock: the on-disk file must equal the fresh reference computation.
     * Line-ending-normalized because {@code core.autocrlf} may rewrite text files to CRLF in a
     * Windows working tree; everything else is byte-exact.
     */
    private static void verifyInPlace(Path path, String content) throws IOException {
        String onDisk = Files.readString(path, StandardCharsets.UTF_8);
        assertEquals(normalize(content), normalize(onDisk),
                path + " drifted from the reference computation - delete the uns-test-vectors/"
                        + " files and re-run this test to regenerate");
    }

    private static String normalize(String s) {
        return s.replace("\r\n", "\n");
    }

    // ===================== README.md =====================

    private static final String README = """
            # UNS cross-language conformance vectors

            These files pin the **normative ggcommons unified-namespace (UNS) grammar** (see
            `docs/platform/UNS-CANONICAL-DESIGN.md` §2.2/§4.1 and `docs/platform/DESIGN-uns.md`):
            topic building, topic validation, subscription filters, the reserved-class publish
            guard, and the golden canonical message envelopes. The Java reference implementation
            generates and verifies them; the Python, Rust, and TypeScript ports **must** pass the
            same conformance checks so every language builds **byte-identical topics** and
            **structurally identical envelopes** (D-U22).

            ## Files

            | File | What it is |
            |------|------------|
            | `topics.json` | `build` / `validate` / `filter` / `guard` case groups (inputs + expected outputs or error codes). |
            | `envelopes.json` | One golden **full canonical JSON** envelope per UNS class, with pinned `uuid`/`correlation_id`/`timestamp`. |
            | `bcast.json` | The `_bcast` **republish** (reconnect-rehydration) contract: the two broadcast command topics, the golden notification envelopes, and the normative listener behavior constants. |

            Both files are UTF-8; some inputs deliberately contain raw C1 control bytes
            (U+0085 etc.) — parse them as JSON, do not preprocess.

            ## topics.json case groups

            Every case is `{name, input, expected}`. Failure cases are **single-fault** (D-U26)
            so all four languages fail with the identical machine-readable code.

            - **build** — input `{hierarchyLevels, identityValues, component, instance,
              includeRoot, class, channel?}` → expected `{topic}` or `{error}`.
              Contract: pair `hierarchyLevels[i]` with `identityValues[<level>]`;
              **`identityValues` and `component` pass through the language's template sanitizer
              first** (`ConfigManager.sanitize` semantics: `/`, `\\`, `+`, `#` and ISO control
              characters — including C1 U+0080–U+009F — each become `_`, then any remaining `..`
              becomes `_`). That models the config identity-resolution path and pins the D-U26
              equivalence "sanitized ⇒ valid". **`instance` and `channel` are used verbatim**
              (they are validated tokens, never sanitized). A missing `channel` key means
              "no channel"; an empty `channel` string also means "no channel".
            - **validate** — input `{topic, includeRoot}` → `{ok: true}` or `{error}`.
              Validation is includeRoot-sensitive (class position 4 rootless / 5 rooted). Bind
              the validator to an identity with a **multi-level hierarchy** (≥ 2 levels) so the
              `includeRoot` input is the effective root mode — D-U25 makes `includeRoot` a no-op
              for single-level hierarchies.
            - **filter** — input `{class, scope{site?, device?, component?, instance?},
              includeRoot}` → `{filter}`. Absent scope fields render as `+`; channeled classes
              get a trailing `/#`; leaf classes (`state`, `cfg`) end at the class token. Same
              multi-level binding note as `validate`.
            - **guard** — input `{topic, includeRoot}` → `{reserved: true|false}`. The §4.1
              reserved-class predicate (D-U24): reserved iff `tokens[0] == "ecv1"` and
              `tokens[4]` (always) or `tokens[5]` (only when `includeRoot` is true) is one of
              `state | metric | cfg | log`. Non-`ecv1` topics always pass.

            Topics and filters compare **byte-for-byte**; error codes compare **exactly** against
            the pinned §2.2 set: `EMPTY_TOKEN, BAD_CHAR, TRAVERSAL, DEPTH_EXCEEDED,
            LENGTH_EXCEEDED, CHANNEL_ON_LEAF, CHANNEL_REQUIRED, BAD_ROOT, BAD_CLASS,
            WILDCARD_IN_TOPIC`.

            ## envelopes.json conformance contract

            Each vector is `{name, class, channel?, topic, envelope}` where `envelope` is the
            full canonical wire JSON `{header, identity, body}`. Every language must:

            1. **Rebuild the envelope** through its message builder with the explicit
               uuid / timestamp / correlation_id setters and the vector's `identity`
               (`envelope.identity` parsed with the lenient wire parser), then assert
               **structural equality** with `envelope` — same key set and values; JSON member
               order is **not** normative (D-U22).
            2. **Reproduce `topic` byte-for-byte** from the vector identity + `class` +
               `channel` with `includeRoot=false` (all envelope vectors are rootless).

            Notes: the two `state` vectors pin the heartbeat-state body shapes — RUNNING carries
            `uptimeSecs`, STOPPED does not (§4.3 / D-U14). The `state`/`cfg` envelope versions
            are pinned to `"1.0"`. Bodies of the other classes are representative payloads (the
            envelope structure is the contract, not the body schema). No envelope carries `tags`
            (built without a config-bound builder) or `reply_to`.

            ## bcast.json republish contract

            Pins the `_bcast` **republish** (reconnect-rehydration) surface — the DESIGN-uns
            §9.3-layer-2 / §9.4 late-join lever the `uns-bridge` drives on every site-reconnect
            rising edge. The document is `{device, commands[], behavior}`:

            - **commands** — exactly two, in order `republish-state`, `republish-cfg`. Each is
              `{name, republishes, topic, input, envelope}`:
              - `topic` is rebuilt **byte-for-byte** from `input`
                (`{device, component: "_bcast", instance: "main", includeRoot: false,
                class: "cmd", channel: <name>}`) through the language's topic builder — the
                reserved `_bcast` pseudo-component pinned to the device, single-level hierarchy,
                so the topic is rootless by D-U25:
                `ecv1/{device}/_bcast/main/cmd/republish-state|republish-cfg`.
              - `envelope` is the golden **notification** the bridge publishes: header
                `{name: <verb>, version: "1.0", timestamp, uuid, correlation_id}`, body `{}` —
                **no `identity`, no `tags`, no `reply_to`** (fire-and-forget). Rebuild through the
                message builder (pinned setters, no identity) and compare **structurally**
                (D-U22).
            - **behavior** — the normative republish-listener constants every language implements:
              `jitterWindowMs` (an accepted broadcast re-announces after a uniformly random delay
              in `[0, jitterWindowMs]`), `cooldownMs` (per verb, at most one re-announce per
              cooldown window, measured from the last **accepted** trigger; everything else
              coalesces), `replyTo: false` (never reply). The listener triggers only when the
              envelope `header.name` equals the topic's verb; malformed/foreign payloads are
              ignored, never crash. `republish-state` re-emits the heartbeat `state` keepalive
              (respecting `heartbeat.enabled`); `republish-cfg` re-runs the effective-config
              (`cfg`) publisher. See `docs/platform/DESIGN-uns.md` §9.4.

            Generated by the Java canonical generator test (D-U12):
            `mvn -f libs/java/pom.xml test -Dtest=UnsTestVectorsGeneratorTest`.
            Do not hand-edit; regenerate by deleting the files and re-running the generator test.
            """;
}
