/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

import com.mbreissi.ggcommons.ParsedCommandLine;
import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.config.ConfigManagerFactory;
import com.mbreissi.ggcommons.messaging.MessageIdentity;
import org.junit.jupiter.api.Test;

import java.io.File;
import java.io.FileWriter;
import java.nio.charset.StandardCharsets;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link Uns#validate(String)}: every one of the ten §2.2 error codes, the depth
 * guard at the exact 7-vs-8-separator boundary, the 256-UTF-8-byte length boundary, rooted vs
 * rootless class positions, build/validate round-trips, and the normative sanitizer/validator
 * reconciliation (any {@code ConfigManager}-sanitized value builds a topic that validates).
 */
class UnsValidateTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private static final Uns ROOTLESS = new Uns(IDENTITY, false);
    private static final Uns ROOTED = new Uns(IDENTITY, true);

    private static UnsValidationException.Code codeOf(org.junit.jupiter.api.function.Executable e) {
        return assertThrows(UnsValidationException.class, e).getCode();
    }

    // ----- the ten error codes, one by one -----

    @Test
    void emptyToken() {
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN, codeOf(() -> ROOTLESS.validate(null)));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN, codeOf(() -> ROOTLESS.validate("")));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.validate("ecv1//opcua-adapter/main/state")));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/state/")));
    }

    @Test
    void badChar() {
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.validate("ecv1/gw\\01/opcua-adapter/main/state")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.validate("ecv1/gw" + (char) 0x01 + "01/c/m/state")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.validate("ecv1/gw" + (char) 0x7F + "01/c/m/state")));
    }

    @Test
    void traversal() {
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.validate("ecv1/../opcua-adapter/main/state")));
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/a..b/main/state")));
    }

    @Test
    void depthExceeded() {
        // 8 '/' separators (9 levels) -> rejected...
        assertEquals(UnsValidationException.Code.DEPTH_EXCEEDED,
                codeOf(() -> ROOTLESS.validate("ecv1/d/c/i/data/a/b/c/d")));
    }

    @Test
    void depthBoundaryExactlySevenSeparatorsPasses() {
        // ...while exactly 7 separators (IoT Core's 8-level limit) is legal.
        assertDoesNotThrow(() -> ROOTLESS.validate("ecv1/d/c/i/data/a/b/c"));
    }

    @Test
    void lengthExceeded() {
        // "ecv1/" + device + "/comp/main/state": 21 fixed chars + device. 235 -> 256 bytes: OK;
        // 236 -> 257 bytes: rejected. (All ASCII, so chars == UTF-8 bytes.)
        String at256 = "ecv1/" + "d".repeat(235) + "/comp/main/state";
        assertEquals(256, at256.getBytes(StandardCharsets.UTF_8).length);
        assertDoesNotThrow(() -> ROOTLESS.validate(at256));

        String at257 = "ecv1/" + "d".repeat(236) + "/comp/main/state";
        assertEquals(UnsValidationException.Code.LENGTH_EXCEEDED,
                codeOf(() -> ROOTLESS.validate(at257)));
    }

    @Test
    void lengthIsMeasuredInUtf8Bytes() {
        // 130 two-byte chars: only 151 chars, but 281 UTF-8 bytes -> over the IoT Core limit.
        String multiByte = "ecv1/" + "é".repeat(130) + "/comp/main/state";
        assertTrue(multiByte.length() < 256);
        assertEquals(UnsValidationException.Code.LENGTH_EXCEEDED,
                codeOf(() -> ROOTLESS.validate(multiByte)));
    }

    @Test
    void channelOnLeaf() {
        assertEquals(UnsValidationException.Code.CHANNEL_ON_LEAF,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/state/extra")));
        assertEquals(UnsValidationException.Code.CHANNEL_ON_LEAF,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/cfg/x")));
    }

    @Test
    void channelRequired() {
        assertEquals(UnsValidationException.Code.CHANNEL_REQUIRED,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/data")));
        assertEquals(UnsValidationException.Code.CHANNEL_REQUIRED,
                codeOf(() -> ROOTED.validate("ecv1/dallas/gw-01/opcua-adapter/main/metric")));
    }

    @Test
    void badRoot() {
        assertEquals(UnsValidationException.Code.BAD_ROOT,
                codeOf(() -> ROOTLESS.validate("notroot/gw-01/opcua-adapter/main/state")));
        // Reply topics are non-UNS by design (D-U6) and must fail as such.
        assertEquals(UnsValidationException.Code.BAD_ROOT,
                codeOf(() -> ROOTLESS.validate("ggcommons/reply-abc/x/y/z")));
    }

    @Test
    void badClass() {
        assertEquals(UnsValidationException.Code.BAD_CLASS,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/bogus/x")));
        // Too few levels: the class position (5th token rootless) does not exist.
        assertEquals(UnsValidationException.Code.BAD_CLASS,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main")));
        // Class tokens are the lowercase wire tokens, not enum names.
        assertEquals(UnsValidationException.Code.BAD_CLASS,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/STATE")));
    }

    @Test
    void wildcardInTopic() {
        assertEquals(UnsValidationException.Code.WILDCARD_IN_TOPIC,
                codeOf(() -> ROOTLESS.validate("ecv1/+/opcua-adapter/main/state")));
        assertEquals(UnsValidationException.Code.WILDCARD_IN_TOPIC,
                codeOf(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/data/#")));
        // The concrete-topics-only rule wins over every other check (even a bad root).
        assertEquals(UnsValidationException.Code.WILDCARD_IN_TOPIC,
                codeOf(() -> ROOTLESS.validate("foo/+/bar")));
    }

    // ----- rooted vs rootless class position -----

    @Test
    void rootedGrammarExpectsSixMinimumTokens() {
        assertDoesNotThrow(() -> ROOTED.validate("ecv1/dallas/gw-01/opcua-adapter/main/state"));
        // A rootless-shaped topic under a rooted validator: no 6th token -> missing class.
        assertEquals(UnsValidationException.Code.BAD_CLASS,
                codeOf(() -> ROOTED.validate("ecv1/gw-01/opcua-adapter/main/state")));
    }

    @Test
    void rootlessGrammarExpectsFiveMinimumTokens() {
        assertDoesNotThrow(() -> ROOTLESS.validate("ecv1/gw-01/opcua-adapter/main/state"));
        // A rooted-shaped topic under a rootless validator: position 4 is 'main', not a class...
        // unless it accidentally parses; here 'dallas'-shifted tokens put 'main' at the class slot.
        assertEquals(UnsValidationException.Code.BAD_CLASS,
                codeOf(() -> ROOTLESS.validate("ecv1/dallas/gw-01/opcua-adapter/main")));
    }

    // ----- build/validate round-trips -----

    @Test
    void builtTopicsAlwaysValidate() {
        assertDoesNotThrow(() -> ROOTLESS.validate(ROOTLESS.topic(UnsClass.STATE)));
        assertDoesNotThrow(() -> ROOTLESS.validate(ROOTLESS.topic(UnsClass.DATA, "temp")));
        assertDoesNotThrow(() -> ROOTLESS.validate(ROOTLESS.topic(UnsClass.CMD, "sb/status")));
        assertDoesNotThrow(() -> ROOTED.validate(ROOTED.topic(UnsClass.STATE)));
        assertDoesNotThrow(() -> ROOTED.validate(ROOTED.topic(UnsClass.EVT, "alarm/high")));
    }

    // ----- sanitizer/validator reconciliation (§2.2 rule 1, Risks #7) -----

    /**
     * Builds a real {@link ConfigManager} through the factory (schema validation + identity
     * resolution + sanitization), exactly like startup does.
     */
    private static ConfigManager managerFor(String thingName) throws Exception {
        File tempFile = File.createTempFile("uns-reconciliation", ".json");
        tempFile.deleteOnExit();
        try (FileWriter writer = new FileWriter(tempFile)) {
            writer.write("{\"component\":{}}");
        }
        ParsedCommandLine cmdLine = new ParsedCommandLine();
        cmdLine.configArgs = new String[]{"FILE", tempFile.getAbsolutePath()};
        cmdLine.thingName = thingName;
        return ConfigManagerFactory.create("com.test.TestComponent", cmdLine);
    }

    @Test
    void sanitizedValueWithASpaceBuildsAndValidates() throws Exception {
        // A thing name with a space survives the sanitizer unchanged; the token rule must accept
        // it (the validator imposes no stricter whitelist than the sanitizer).
        ConfigManager cm = managerFor("gw 01");
        try {
            Uns uns = new Uns(cm.getComponentIdentity(), cm.isTopicIncludeRoot());
            String topic = uns.topic(UnsClass.STATE);
            assertEquals("ecv1/gw 01/TestComponent/main/state", topic);
            assertDoesNotThrow(() -> uns.validate(topic));
        } finally {
            cm.close();
        }
    }

    @Test
    void valueWithAPlusWasSanitizedAndThereforeBuilds() throws Exception {
        // A thing name with '+' is sanitized to '_' by ConfigManager -> the identity value the
        // builder sees is already publishable.
        ConfigManager cm = managerFor("gw+01");
        try {
            Uns uns = new Uns(cm.getComponentIdentity(), cm.isTopicIncludeRoot());
            String topic = uns.topic(UnsClass.STATE);
            assertEquals("ecv1/gw_01/TestComponent/main/state", topic);
            assertDoesNotThrow(() -> uns.validate(topic));
        } finally {
            cm.close();
        }
    }

    @Test
    void unsanitizedValueIsExactlyWhatTheTokenRuleRejects() {
        // The same '+' value NOT passed through the sanitizer must fail the token rule — pinning
        // that the two rule sets are the same blacklist (tighten one -> tighten both).
        MessageIdentity unsanitized = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw+01")), "comp", "main");
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> new Uns(unsanitized, false).topic(UnsClass.STATE)));
    }

    // ----- checkToken (the shared §2.2 token rule) -----

    @Test
    void checkTokenEnforcesTheSanitizerBlacklist() {
        assertDoesNotThrow(() -> Uns.checkToken("kep1", "instance id"));
        assertDoesNotThrow(() -> Uns.checkToken("with space", "instance id"));
        assertDoesNotThrow(() -> Uns.checkToken("v1.2", "instance id"));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> Uns.checkToken(null, "instance id")));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> Uns.checkToken("", "instance id")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> Uns.checkToken("a/b", "instance id")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> Uns.checkToken("a+b", "instance id")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> Uns.checkToken("a#b", "instance id")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> Uns.checkToken("a\\b", "instance id")));
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> Uns.checkToken("a..b", "instance id")));
    }
}
