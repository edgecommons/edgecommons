/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

import com.mbreissi.ggcommons.messaging.MessageIdentity;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Set;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link Uns} topic building ({@code topic}/{@code topicFor}), the
 * {@link UnsClass} enum contract, the §2.2 build-time rules (token rule, leaf/channel rules,
 * depth guard at the boundary, UTF-8 length limit) and {@code topic.includeRoot} on/off.
 */
class UnsTopicTest {

    private static final MessageIdentity SINGLE = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private static final MessageIdentity MULTI = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("zone", "zone-3"),
                    new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private static final Uns ROOTLESS = new Uns(SINGLE, false);
    private static final Uns ROOTED = new Uns(MULTI, true);

    private static UnsValidationException.Code codeOf(org.junit.jupiter.api.function.Executable e) {
        return assertThrows(UnsValidationException.class, e).getCode();
    }

    // ----- UnsClass enum contract (§2.1) -----

    @Test
    void classTokensAndLeafFlags() {
        assertEquals("state", UnsClass.STATE.token);
        assertEquals("metric", UnsClass.METRIC.token);
        assertEquals("cfg", UnsClass.CFG.token);
        assertEquals("log", UnsClass.LOG.token);
        assertEquals("data", UnsClass.DATA.token);
        assertEquals("evt", UnsClass.EVT.token);
        assertEquals("cmd", UnsClass.CMD.token);
        assertEquals("app", UnsClass.APP.token);
        // Leaf classes forbid a channel; all others require one.
        assertTrue(UnsClass.STATE.leaf);
        assertTrue(UnsClass.CFG.leaf);
        for (UnsClass cls : Set.of(UnsClass.METRIC, UnsClass.LOG, UnsClass.DATA,
                UnsClass.EVT, UnsClass.CMD, UnsClass.APP)) {
            assertFalse(cls.leaf, cls + " must be channeled");
        }
    }

    @Test
    void reservedSetIsTheLibraryOwnedClasses() {
        assertEquals(Set.of(UnsClass.STATE, UnsClass.METRIC, UnsClass.CFG, UnsClass.LOG),
                UnsClass.RESERVED);
    }

    @Test
    void fromTokenResolvesEveryClassAndRejectsUnknown() {
        for (UnsClass cls : UnsClass.values()) {
            assertSame(cls, UnsClass.fromToken(cls.token));
        }
        assertNull(UnsClass.fromToken("bogus"));
        assertNull(UnsClass.fromToken("STATE")); // wire tokens are lowercase
    }

    // ----- topic(): happy paths -----

    @Test
    void leafTopics() {
        assertEquals("ecv1/gw-01/opcua-adapter/main/state", ROOTLESS.topic(UnsClass.STATE));
        assertEquals("ecv1/gw-01/opcua-adapter/main/cfg", ROOTLESS.topic(UnsClass.CFG));
    }

    @Test
    void channeledTopicSingleToken() {
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/temp",
                ROOTLESS.topic(UnsClass.DATA, "temp"));
    }

    @Test
    void channeledTopicMultiTokenChannel() {
        assertEquals("ecv1/gw-01/opcua-adapter/main/cmd/sb/status",
                ROOTLESS.topic(UnsClass.CMD, "sb/status"));
    }

    @Test
    void deviceIsTheLastHierValueNotTheFirst() {
        // A multi-level hierarchy without includeRoot still emits ONLY the device level.
        assertEquals("ecv1/gw-01/opcua-adapter/main/state",
                new Uns(MULTI, false).topic(UnsClass.STATE));
    }

    @Test
    void instanceTokenFlowsIntoTheTopic() {
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/state",
                new Uns(SINGLE.withInstance("kep1"), false).topic(UnsClass.STATE));
    }

    @Test
    void identityAccessorReturnsTheBoundIdentity() {
        assertSame(SINGLE, ROOTLESS.identity());
        assertSame(MULTI, ROOTED.identity());
    }

    // ----- topic.includeRoot -----

    @Test
    void includeRootInsertsFirstHierValueAfterTheRoot() {
        assertEquals("ecv1/dallas/gw-01/opcua-adapter/main/state", ROOTED.topic(UnsClass.STATE));
        assertEquals("ecv1/dallas/gw-01/opcua-adapter/main/data/temp",
                ROOTED.topic(UnsClass.DATA, "temp"));
    }

    @Test
    void includeRootWithSingleLevelHierarchyIsANoOp() {
        // D-U25: with the zero-config ["device"] hierarchy, hier[0] IS the device — prepending
        // it would duplicate the device level (ecv1/gw-01/gw-01/…), so includeRoot is a no-op.
        assertEquals("ecv1/gw-01/opcua-adapter/main/state",
                new Uns(SINGLE, true).topic(UnsClass.STATE));
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/temp",
                new Uns(SINGLE, true).topic(UnsClass.DATA, "temp"));
    }

    @Test
    void includeRootNoOpRestoresTheRootlessChannelBudget() {
        // D-U25: the effective root mode is rootless, so the channel budget is 3 tokens again.
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/a/b/c",
                new Uns(SINGLE, true).topic(UnsClass.DATA, "a/b/c"));
        assertEquals(UnsValidationException.Code.DEPTH_EXCEEDED,
                codeOf(() -> new Uns(SINGLE, true).topic(UnsClass.DATA, "a/b/c/d")));
    }

    @Test
    void topicForSingleLevelTargetUnderARootedInstanceIsAlsoANoOp() {
        // D-U25 is a property of the identity being minted: a rooted (multi-level) component
        // addressing a single-level-hierarchy peer must not duplicate the peer's device.
        MessageIdentity peer = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-02")),
                "modbus-adapter", "main");
        assertEquals("ecv1/gw-02/modbus-adapter/main/cmd/set-config",
                ROOTED.topicFor(peer, UnsClass.CMD, "set-config"));
    }

    // ----- leaf / channel rules -----

    @Test
    void channelOnLeafThrows() {
        assertEquals(UnsValidationException.Code.CHANNEL_ON_LEAF,
                codeOf(() -> ROOTLESS.topic(UnsClass.STATE, "x")));
        assertEquals(UnsValidationException.Code.CHANNEL_ON_LEAF,
                codeOf(() -> ROOTLESS.topic(UnsClass.CFG, "a/b")));
    }

    @Test
    void channelRequiredThrows() {
        assertEquals(UnsValidationException.Code.CHANNEL_REQUIRED,
                codeOf(() -> ROOTLESS.topic(UnsClass.METRIC)));
        assertEquals(UnsValidationException.Code.CHANNEL_REQUIRED,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, null)));
        // An empty channel string means "no channel".
        assertEquals(UnsValidationException.Code.CHANNEL_REQUIRED,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "")));
    }

    // ----- token rule on channel tokens (§2.2 rule 1) -----

    @Test
    void emptyChannelTokensThrow() {
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a//b")));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "/a")));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a/")));
    }

    @Test
    void badCharactersInChannelTokensThrow() {
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "te+mp")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "#")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a\\b")));
        // ISO control characters (D-U26, the exact sanitizer predicate): C0 U+0000-U+001F,
        // U+007F (DEL), AND C1 U+0080-U+009F.
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0x01 + "b")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0x7F + "b")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a\nb")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0x80 + "b")));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0x85 + "b"))); // NEL
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0x9F + "b")));
        // The first char OUTSIDE the C1 range (U+00A0 NBSP) must pass again (the boundary).
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/a" + (char) 0xA0 + "b",
                ROOTLESS.topic(UnsClass.DATA, "a" + (char) 0xA0 + "b"));
    }

    @Test
    void traversalInChannelTokensThrows() {
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a..b")));
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "..")));
    }

    @Test
    void dotsAndSpacesAreLegalInTokens() {
        // D5: dots are a literal within a level; spaces survive the template sanitizer, so the
        // token rule must accept them (no stricter whitelist than the sanitizer).
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/v1.2",
                ROOTLESS.topic(UnsClass.DATA, "v1.2"));
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/line 1",
                ROOTLESS.topic(UnsClass.DATA, "line 1"));
    }

    // ----- depth guard at the boundary (§2.2 rule 2) -----

    @Test
    void rootlessChannelBudgetIsThreeTokens() {
        String topic = ROOTLESS.topic(UnsClass.DATA, "a/b/c"); // exactly 7 '/' separators
        assertEquals(7, topic.chars().filter(c -> c == '/').count());
        assertEquals("ecv1/gw-01/opcua-adapter/main/data/a/b/c", topic);
        assertEquals(UnsValidationException.Code.DEPTH_EXCEEDED,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "a/b/c/d"))); // 8 separators
    }

    @Test
    void rootedChannelBudgetIsTwoTokens() {
        String topic = ROOTED.topic(UnsClass.DATA, "a/b"); // exactly 7 '/' separators
        assertEquals(7, topic.chars().filter(c -> c == '/').count());
        assertEquals("ecv1/dallas/gw-01/opcua-adapter/main/data/a/b", topic);
        assertEquals(UnsValidationException.Code.DEPTH_EXCEEDED,
                codeOf(() -> ROOTED.topic(UnsClass.DATA, "a/b/c"))); // 8 separators
    }

    // ----- length limit at the boundary (§2.2 rule 3) -----

    @Test
    void lengthBoundaryIs256Utf8Bytes() {
        // Measure the fixed prefix, then craft a channel token that lands exactly on the limit.
        String prefix = ROOTLESS.topic(UnsClass.DATA, "x"); // ASCII: chars == bytes
        int room = Uns.MAX_TOPIC_UTF8_BYTES - (prefix.length() - 1);
        String atLimit = ROOTLESS.topic(UnsClass.DATA, "x".repeat(room));
        assertEquals(Uns.MAX_TOPIC_UTF8_BYTES, atLimit.getBytes(StandardCharsets.UTF_8).length);
        assertEquals(UnsValidationException.Code.LENGTH_EXCEEDED,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "x".repeat(room + 1))));
    }

    @Test
    void lengthIsMeasuredInUtf8BytesNotChars() {
        // 120 two-byte characters: 145 chars total but 265 UTF-8 bytes -> over the limit.
        assertEquals(UnsValidationException.Code.LENGTH_EXCEEDED,
                codeOf(() -> ROOTLESS.topic(UnsClass.DATA, "é".repeat(120))));
    }

    // ----- topicFor (peer addressing) -----

    @Test
    void topicForMintsTheTargetsTokens() {
        MessageIdentity peer = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-02")),
                "modbus-adapter", "plc7");
        assertEquals("ecv1/gw-02/modbus-adapter/plc7/cmd/set-config",
                ROOTLESS.topicFor(peer, UnsClass.CMD, "set-config"));
        assertEquals("ecv1/gw-02/modbus-adapter/plc7/state",
                ROOTLESS.topicFor(peer, UnsClass.STATE, null));
    }

    @Test
    void topicForUnderIncludeRootUsesTheTargetsFirstHierValue() {
        MessageIdentity peer = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("site", "austin"),
                        new MessageIdentity.HierEntry("device", "gw-02")),
                "modbus-adapter", "main");
        assertEquals("ecv1/austin/gw-02/modbus-adapter/main/cmd/get-configuration",
                ROOTED.topicFor(peer, UnsClass.CMD, "get-configuration"));
    }

    @Test
    void topicForRejectsUnsanitizedForeignIdentityTokens() {
        // A wire identity is lenient (MessageIdentity.fromDict) and may carry values the
        // sanitizer never saw — the builder must refuse to mint an unpublishable topic.
        MessageIdentity badDevice = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw+02")), "comp", "main");
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topicFor(badDevice, UnsClass.STATE, null)));

        MessageIdentity badComponent = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-02")), "co#mp", "main");
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.topicFor(badComponent, UnsClass.STATE, null)));

        MessageIdentity badInstance = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-02")), "comp", "in..st");
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.topicFor(badInstance, UnsClass.STATE, null)));
    }

    // ----- null handling -----

    @Test
    void nullArgumentsAreRejected() {
        assertThrows(NullPointerException.class, () -> new Uns(null, false));
        assertThrows(NullPointerException.class, () -> ROOTLESS.topic(null));
        assertThrows(NullPointerException.class, () -> ROOTLESS.topicFor(null, UnsClass.STATE, null));
        assertThrows(NullPointerException.class, () -> ROOTLESS.topicFor(SINGLE, null, null));
    }
}
