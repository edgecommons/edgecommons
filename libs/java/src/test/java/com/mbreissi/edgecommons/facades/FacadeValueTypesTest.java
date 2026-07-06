/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for the facade value types ({@link Channel}, {@link Quality}, {@link Severity},
 * {@link SignalUpdate}/{@link SignalUpdate.Sample}) — the parts the mirrors replicate verbatim.
 */
class FacadeValueTypesTest {

    // ===================== Channel =====================

    @Test
    void channelFromConfigParsesEveryRecognizedForm() {
        assertSame(Channel.LOCAL, Channel.fromConfig("local"));
        assertSame(Channel.LOCAL, Channel.fromConfig("LOCAL"));
        assertSame(Channel.NORTHBOUND, Channel.fromConfig("northbound"));
        assertEquals(Channel.Kind.STREAM, Channel.fromConfig("stream:hot").kind());
        assertEquals("hot", Channel.fromConfig("stream:hot").streamName());
    }

    @Test
    void channelFromConfigYieldsNullForAbsentOrUnrecognized() {
        assertNull(Channel.fromConfig(null));
        assertNull(Channel.fromConfig(""));
        assertNull(Channel.fromConfig("   "));
        assertNull(Channel.fromConfig("bogus"));
        assertNull(Channel.fromConfig("iotcore"));
        assertNull(Channel.fromConfig("iot_core"));
        assertNull(Channel.fromConfig("stream:"), "an empty stream name is not a valid channel");
    }

    @Test
    void channelStreamRejectsEmptyName() {
        assertThrows(IllegalArgumentException.class, () -> Channel.stream(""));
        assertThrows(IllegalArgumentException.class, () -> Channel.stream(null));
    }

    @Test
    void channelEqualityAndStringForm() {
        assertEquals(Channel.LOCAL, Channel.LOCAL);
        assertEquals(Channel.stream("hot"), Channel.stream("hot"));
        assertNotEquals(Channel.stream("hot"), Channel.stream("cold"));
        assertNotEquals(Channel.LOCAL, Channel.NORTHBOUND);
        assertNotEquals(Channel.LOCAL, "local");
        assertEquals(Channel.stream("hot").hashCode(), Channel.stream("hot").hashCode());
        assertEquals("local", Channel.LOCAL.toString());
        assertEquals("northbound", Channel.NORTHBOUND.toString());
        assertEquals("stream:hot", Channel.stream("hot").toString());
    }

    // ===================== Quality / Severity =====================

    @Test
    void qualityWireTokens() {
        assertEquals("GOOD", Quality.GOOD.wire());
        assertEquals("BAD", Quality.BAD.wire());
        assertEquals("UNCERTAIN", Quality.UNCERTAIN.wire());
        assertEquals(Quality.GOOD, Quality.fromWire("GOOD"));
        assertNull(Quality.fromWire("good"), "wire tokens are UPPERCASE");
        assertNull(Quality.fromWire("nope"));
    }

    @Test
    void severityWireTokens() {
        assertEquals("critical", Severity.CRITICAL.wire());
        assertEquals(Severity.INFO, Severity.fromWire("info"));
        assertNull(Severity.fromWire("INFO"), "wire tokens are lowercase");
    }

    // ===================== SignalUpdate / Sample =====================

    @Test
    void sampleFactoriesSetTheExpectedFields() {
        SignalUpdate.Sample a = SignalUpdate.Sample.of(1.0);
        assertEquals(1.0, a.value());
        assertNull(a.quality());

        SignalUpdate.Sample b = SignalUpdate.Sample.of(2, Quality.BAD);
        assertEquals(Quality.BAD, b.quality());
        assertNull(b.sourceTs());

        SignalUpdate.Sample c = SignalUpdate.Sample.of(3, Quality.UNCERTAIN, "2026-01-01T00:00:00Z");
        assertEquals("2026-01-01T00:00:00Z", c.sourceTs());
    }

    @Test
    void signalUpdateBuilderAccessors() {
        JsonObject address = new JsonObject();
        address.addProperty("ns", 2);
        SignalUpdate u = new SignalUpdate.Builder("sig-1")
                .name("Signal One")
                .address(address)
                .addSample(1.0)
                .build();
        assertEquals("sig-1", u.signalId());
        assertEquals("Signal One", u.signalName());
        assertEquals(address, u.signalAddress());
        assertEquals("sig-1", u.effectiveSignalPath(), "signalPath defaults to signalId");
        assertNull(u.via());
        assertEquals(1, u.samples().size());
        assertTrue(u.device() == null);

        SignalUpdate withPath = new SignalUpdate.Builder("sig-1")
                .signalPath("a/b").via(Channel.NORTHBOUND).addSamples(u.samples()).build();
        assertEquals("a/b", withPath.effectiveSignalPath());
        assertEquals(Channel.NORTHBOUND, withPath.via());
    }
}
