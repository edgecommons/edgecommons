/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

import static org.junit.jupiter.api.Assumptions.assumeTrue;

/**
 * The Java loader for the cross-language {@code uns-test-vectors/} conformance suite — the model
 * the Python/Rust/TS loaders mirror (UNS-CANONICAL-DESIGN §7 / D-U12/D-U13). Reads the committed
 * vector files and replays every case through the live implementation via {@link UnsTestVectors}:
 * every build/validate/filter/guard case must match byte-for-byte (topics/filters) or by exact
 * error code, and every golden envelope must be reproduced by {@link MessageBuilder} and equal
 * the file structurally (member order is not normative, D-U22).
 *
 * <p>Existence-guarded like the vault loader ({@code VaultTest.crossLanguageConformance}): skips
 * when the vector directory is absent (the generator test creates it on first run; the files are
 * committed, so CI always exercises this).
 */
class UnsTestVectorsLoaderTest {

    @Test
    void crossLanguageTopicsConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("topics.json"));
        UnsTestVectors.assertTopicsDocument(doc);
    }

    @Test
    void crossLanguageEnvelopesConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("envelopes.json"));
        UnsTestVectors.assertEnvelopesDocument(doc);
    }

    @Test
    void crossLanguageBcastConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("bcast.json"));
        UnsTestVectors.assertBcastDocument(doc);
    }

    @Test
    void crossLanguageCommandsConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("commands.json"));
        UnsTestVectors.assertCommandsDocument(doc);
    }

    @Test
    void crossLanguageDataConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("data.json"));
        UnsTestVectors.assertDataDocument(doc);
    }

    @Test
    void crossLanguageEvtConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("evt.json"));
        UnsTestVectors.assertEvtDocument(doc);
    }

    @Test
    void crossLanguageAppConformance() throws Exception {
        JsonObject doc = load(UnsTestVectors.DIR.resolve("app.json"));
        UnsTestVectors.assertAppDocument(doc);
    }

    /** Skips (never fails) when the shared vector files have not been generated yet. */
    private static JsonObject load(Path path) throws Exception {
        assumeTrue(Files.exists(path), "uns-test-vectors not present (" + path + ")");
        return JsonParser.parseString(Files.readString(path, StandardCharsets.UTF_8))
                .getAsJsonObject();
    }
}
