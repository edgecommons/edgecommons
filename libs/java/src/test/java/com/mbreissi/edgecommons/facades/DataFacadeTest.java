/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.test.MockConfigurationService;
import com.mbreissi.edgecommons.test.MockMessagingService;
import com.mbreissi.edgecommons.uns.Uns;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import com.mbreissi.edgecommons.messaging.Qos;

import java.time.Clock;
import java.time.Instant;
import java.time.ZoneOffset;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Deterministic unit tests for {@link DataFacade} — the {@code data()} publish facade
 * (DESIGN-class-facades §2.1, D2/D5): the {@code SouthboundSignalUpdate} body construction +
 * defaulting (quality → {@code GOOD} + {@code qualityRaw:"unspecified"}, {@code serverTs} → now),
 * the missing-{@code signal.id} reject, the raw escape hatch, and the local/northbound/stream
 * channel routing. Time is a fixed injected {@link Clock} so {@code serverTs} defaults are pinned.
 */
class DataFacadeTest {

    private static final String NOW = "2026-07-01T12:00:00Z";
    private static final Clock CLOCK = Clock.fixed(Instant.parse(NOW), ZoneOffset.UTC);
    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private MockMessagingService messaging;

    /** A capturing {@link StreamSink} for the stream-route tests. */
    private static final class RecordingSink implements StreamSink {
        String streamName;
        String partitionKey;
        long timestampMs;
        byte[] payload;
        int count;

        @Override
        public void append(String streamName, String partitionKey, long timestampMs, byte[] payload) {
            this.streamName = streamName;
            this.partitionKey = partitionKey;
            this.timestampMs = timestampMs;
            this.payload = payload;
            this.count++;
        }
    }

    @BeforeEach
    void setUp() {
        messaging = new MockMessagingService();
    }

    private DataFacade facade() {
        return facade(new MockConfigurationService(), null);
    }

    private DataFacade facade(MockConfigurationService config, StreamSink sink) {
        config.setComponentIdentity(IDENTITY);
        Uns uns = new Uns(IDENTITY.withInstance("kep1"), false);
        return new DataFacade(config, "kep1", uns, messaging, sink, CLOCK);
    }

    private JsonObject lastBody() {
        List<MockMessagingService.PublishedMessage> published = messaging.getPublishedMessages();
        return published.get(published.size() - 1).message.toDict().getAsJsonObject("body");
    }

    private JsonObject firstSample(JsonObject body) {
        return body.getAsJsonArray("samples").get(0).getAsJsonObject();
    }

    // ===================== defaulting =====================

    @Test
    void qualityDefaultsToGoodWithUnspecifiedMarkerAndServerTsNow() {
        facade().publish("temp", 21.5);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/temp", pm.topic);
        assertNull(pm.qos, "LOCAL route is the default (no QoS)");
        JsonObject sample = firstSample(lastBody());
        assertEquals(21.5, sample.get("value").getAsDouble());
        assertEquals("GOOD", sample.get("quality").getAsString());
        assertEquals("unspecified", sample.get("qualityRaw").getAsString(),
                "a defaulted quality carries the synthetic marker so consumers can tell it apart");
        assertEquals(NOW, sample.get("serverTs").getAsString());
        assertFalse(sample.has("sourceTs"), "sourceTs is never synthesized");
    }

    @Test
    void explicitQualityIsNotMarkedUnspecified() {
        facade().publish("temp", 0, Quality.BAD);

        JsonObject sample = firstSample(lastBody());
        assertEquals("BAD", sample.get("quality").getAsString());
        assertFalse(sample.has("qualityRaw"),
                "an explicit quality with no qualityRaw stays unmarked (not 'unspecified')");
    }

    @Test
    void explicitQualityRawIsPassedThroughVerbatim() {
        facade().signal("temp")
                .addSample(new SignalUpdate.Sample(21.5, Quality.GOOD, "Good", "2026-07-01T11:00:00Z",
                        "2026-07-01T11:00:01Z"))
                .publish();

        JsonObject sample = firstSample(lastBody());
        assertEquals("Good", sample.get("qualityRaw").getAsString());
        assertEquals("2026-07-01T11:00:00Z", sample.get("sourceTs").getAsString());
        assertEquals("2026-07-01T11:00:01Z", sample.get("serverTs").getAsString(),
                "a caller-supplied serverTs is not overwritten by now");
    }

    @Test
    void fluentBuilderConstructsTheFullSouthboundBody() {
        JsonObject address = JsonParser.parseString("{\"ns\":2,\"nodeId\":\"Line1.Temp\"}")
                .getAsJsonObject();
        facade().signal("ns=2;s=Line1.Temp")
                .name("Line 1 Temperature")
                .address(address)
                .device("opcua", "kep1", "opc.tcp://host:4840")
                .addSample(21.5)
                .signalPath("press12/temperature")
                .publish();

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/press12/temperature", pm.topic);
        JsonObject body = lastBody();
        assertEquals("opcua", body.getAsJsonObject("device").get("adapter").getAsString());
        assertEquals("ns=2;s=Line1.Temp", body.getAsJsonObject("signal").get("id").getAsString());
        assertEquals("Line 1 Temperature", body.getAsJsonObject("signal").get("name").getAsString());
        assertEquals(2, body.getAsJsonObject("signal").getAsJsonObject("address").get("ns").getAsInt());
    }

    @Test
    void batchSamplesArePublishedInOrder() {
        facade().signal("flow")
                .addSample(1.0)
                .addSample(2.0, Quality.UNCERTAIN)
                .publish();

        assertEquals(2, lastBody().getAsJsonArray("samples").size());
    }

    // ===================== rejects (the only hard failures) =====================

    @Test
    void missingSignalIdIsRejected() {
        DataFacade facade = facade();
        SignalUpdate update = new SignalUpdate.Builder((String) null).addSample(1.0).build();
        assertThrows(IllegalArgumentException.class, () -> facade.publish(update));
        assertTrue(messaging.getPublishedMessages().isEmpty(), "nothing reaches the wire");
    }

    @Test
    void emptySamplesIsRejected() {
        DataFacade facade = facade();
        SignalUpdate update = new SignalUpdate.Builder("temp").build();
        assertThrows(IllegalArgumentException.class, () -> facade.publish(update));
    }

    @Test
    void quailtyOnlySampleWithNoValueIsRejected() {
        DataFacade facade = facade();
        SignalUpdate update = new SignalUpdate.Builder("temp")
                .addSample(new SignalUpdate.Sample(null, Quality.BAD, null, null, null)).build();
        assertThrows(IllegalArgumentException.class, () -> facade.publish(update));
    }

    @Test
    void detachedBuilderPublishThrows() {
        SignalUpdate.Builder detached = new SignalUpdate.Builder("temp").addSample(1.0);
        assertThrows(IllegalStateException.class, detached::publish);
    }

    // ===================== channel sanitization =====================

    @Test
    void channelPathIsSanitized() {
        facade().publish("a+b", 1.0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/a_b",
                messaging.getPublishedMessages().get(0).topic);
    }

    @Test
    void multiTokenSignalPathBecomesMultipleChannelTokens() {
        facade().publish("a/b", 1.0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/a/b",
                messaging.getPublishedMessages().get(0).topic);
    }

    // ===================== raw escape hatch =====================

    @Test
    void rawEscapeHatchPublishesBodyVerbatim() {
        JsonObject raw = JsonParser.parseString("{\"anything\":\"goes\",\"n\":7}").getAsJsonObject();
        facade().publishBody("custom", raw);

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/custom", pm.topic);
        assertEquals(raw, pm.message.toDict().getAsJsonObject("body"),
                "the escape hatch applies no defaulting - body rides verbatim");
    }

    // ===================== channel routing =====================

    @Test
    void northboundOverrideRoutesToIoTCore() {
        facade().signal("temp").addSample(21.5).via(Channel.NORTHBOUND).publish();

        MockMessagingService.PublishedMessage pm = messaging.getPublishedMessages().get(0);
        assertEquals(Qos.AT_LEAST_ONCE, pm.qos, "northbound uses publishNorthbound");
    }

    @Test
    void streamOverrideAppendsToTheStreamWithSignalIdPartitionKey() {
        RecordingSink sink = new RecordingSink();
        facade(new MockConfigurationService(), sink)
                .signal("ns=2;s=Line1.Temp").addSample(21.5).via(Channel.stream("hot")).publish();

        assertEquals(1, sink.count, "the record went to the stream, not the bus");
        assertTrue(messaging.getPublishedMessages().isEmpty());
        assertEquals("hot", sink.streamName);
        assertEquals("ns=2;s=Line1.Temp", sink.partitionKey, "partition key is the stable signal.id");
        assertEquals(Instant.parse(NOW).toEpochMilli(), sink.timestampMs);
        JsonObject env = Message.fromBytes(sink.payload).toDict();
        assertEquals("ns=2;s=Line1.Temp",
                env.getAsJsonObject("body").getAsJsonObject("signal").get("id").getAsString(),
                "the streamed payload is the same enriched envelope the bus would carry");
    }

    @Test
    void streamRouteFallsBackToLocalWhenNoStreamingConfigured() {
        // facade() wires a null StreamSink (streaming not configured).
        facade().signal("temp").addSample(21.5).via(Channel.stream("hot")).publish();

        assertEquals(1, messaging.getPublishedMessages().size(),
                "readiness/no-streaming -> local: the record falls back to a LOCAL publish");
        assertNull(messaging.getPublishedMessages().get(0).qos);
    }

    @Test
    void configuredInstancePublishChannelRoutesWithoutAnOverride() {
        MockConfigurationService config = new MockConfigurationService();
        config.setFullConfig(JsonParser.parseString(
                "{\"component\":{\"instances\":[{\"id\":\"kep1\","
                        + "\"publish\":{\"channel\":\"northbound\"}}]}}").getAsJsonObject());
        facade(config, null).publish("temp", 21.5);

        assertEquals(Qos.AT_LEAST_ONCE, messaging.getPublishedMessages().get(0).qos,
                "config publish.channel=northbound routes northbound with no per-call override");
    }

    @Test
    void configuredGlobalPublishChannelIsTheFallbackDefault() {
        MockConfigurationService config = new MockConfigurationService();
        config.setFullConfig(JsonParser.parseString(
                "{\"component\":{\"global\":{\"publish\":{\"channel\":\"northbound\"}}}}")
                .getAsJsonObject());
        facade(config, null).publish("temp", 21.5);

        assertEquals(Qos.AT_LEAST_ONCE, messaging.getPublishedMessages().get(0).qos);
    }

    @Test
    void perCallOverrideWinsOverConfigDefault() {
        MockConfigurationService config = new MockConfigurationService();
        config.setFullConfig(JsonParser.parseString(
                "{\"component\":{\"instances\":[{\"id\":\"kep1\","
                        + "\"publish\":{\"channel\":\"northbound\"}}]}}").getAsJsonObject());
        facade(config, null).signal("temp").addSample(21.5).via(Channel.LOCAL).publish();

        assertNull(messaging.getPublishedMessages().get(0).qos,
                "an explicit via(LOCAL) beats the config northbound default");
    }

    @Test
    void resolveChannelPrecedence() {
        DataFacade facade = facade();
        assertEquals(Channel.NORTHBOUND, facade.resolveChannel(Channel.NORTHBOUND));
        assertEquals(Channel.LOCAL, facade.resolveChannel(null));
        assertEquals("kep1", facade.instanceId());
    }

    @Test
    void unrecognizedConfigChannelFallsThroughToLocal() {
        MockConfigurationService config = new MockConfigurationService();
        config.setFullConfig(JsonParser.parseString(
                "{\"component\":{\"instances\":[{\"id\":\"kep1\","
                        + "\"publish\":{\"channel\":\"bogus\"}}]}}").getAsJsonObject());
        facade(config, null).publish("temp", 1.0);
        assertNull(messaging.getPublishedMessages().get(0).qos, "an unparseable channel -> LOCAL");
    }

    // ===================== transport-failure isolation (readiness stays local) =====================

    @Test
    void northboundTransportFailureIsSwallowed() {
        messaging = new MockMessagingService() {
            @Override
            public void publishNorthbound(String topic,
                    com.mbreissi.edgecommons.messaging.Message message, Qos qos) {
                throw new RuntimeException("iot core down");
            }
        };
        MockConfigurationService config = new MockConfigurationService();
        config.setComponentIdentity(IDENTITY);
        Uns uns = new Uns(IDENTITY.withInstance("kep1"), false);
        DataFacade facade = new DataFacade(config, "kep1", uns, messaging, null, CLOCK);
        // A northbound outage must NOT propagate (it would otherwise flip local readiness).
        facade.signal("temp").addSample(1.0).via(Channel.NORTHBOUND).publish();
    }

    @Test
    void streamAppendFailureIsSwallowed() {
        StreamSink throwing = (name, pk, ts, payload) -> {
            throw new RuntimeException("stream buffer full");
        };
        // A stream-append outage must NOT propagate either.
        facade(new MockConfigurationService(), throwing)
                .signal("temp").addSample(1.0).via(Channel.stream("hot")).publish();
        assertTrue(messaging.getPublishedMessages().isEmpty(), "it tried the stream, not the bus");
    }
}
