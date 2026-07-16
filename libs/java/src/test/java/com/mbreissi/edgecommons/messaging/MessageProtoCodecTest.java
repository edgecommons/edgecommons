package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonArray;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.protobuf.ByteString;
import com.mbreissi.edgecommons.messaging.proto.MessageBodyCase;
import com.mbreissi.edgecommons.messaging.proto.MessageBodySchema;
import com.mbreissi.edgecommons.messaging.proto.MessageProtoCodec;
import com.mbreissi.edgecommons.proto.v1.EdgeCommonsMessage;
import com.mbreissi.edgecommons.proto.v1.Header;
import com.mbreissi.edgecommons.proto.v1.Identity;
import org.junit.jupiter.api.Test;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Base64;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class MessageProtoCodecTest {

    @Test
    void componentScopeIdentityOmitsTheInstanceOnTheWireAndInstanceScopePreservesIt() {
        // D‑U28: the identity's instance slot is optional. A component-scope identity (no instance)
        // must survive the protobuf round trip carrying no instance; an instance-scope identity must
        // carry its token back verbatim.
        MessageIdentity component = new MessageIdentity(List.of(
                new MessageIdentity.HierEntry("site", "dallas"),
                new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", null);
        MessageIdentity instance = component.withInstance("kep1");

        JsonObject payload = new JsonObject();
        payload.addProperty("v", 1);

        Message componentDecoded = Message.fromBytes(MessageBuilder.create("Evt", "1.0")
                .withStructuredPayload(payload).withIdentity(component).build().toBytes());
        assertNull(componentDecoded.getIdentity().getInstance(),
                "a component-scope identity carries no instance across the wire");
        assertEquals("opcua-adapter", componentDecoded.getIdentity().getComponent());
        assertEquals("dallas/gw-01", componentDecoded.getIdentity().getPath());

        Message instanceDecoded = Message.fromBytes(MessageBuilder.create("Evt", "1.0")
                .withStructuredPayload(payload).withIdentity(instance).build().toBytes());
        assertEquals("kep1", instanceDecoded.getIdentity().getInstance(),
                "an instance-scope identity round-trips its instance token");
    }

    @Test
    void structuredBodyRoundTripsThroughBytes() {
        JsonObject payload = new JsonObject();
        payload.addProperty("temperature", 21.5);
        payload.addProperty("ok", true);
        JsonObject nested = new JsonObject();
        nested.addProperty("line", "A");
        payload.add("nested", nested);

        Message message = MessageBuilder.create("StructuredSample", "1.0")
                .withTimestampMs(1783360800000L)
                .withUuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1155")
                .withStructuredPayload(payload)
                .build();
        JsonObject tags = new JsonObject();
        tags.addProperty("retention", "short");
        tags.addProperty("priority", 5);
        message = MessageBuilder.create(message.getHeader().getName(), message.getHeader().getVersion())
                .withTimestampMs(message.getHeader().getTimestampMs())
                .withUuid(message.getHeader().getUuid())
                .withStructuredPayload(payload)
                .withTags(MessageTags.fromDict(tags))
                .build();

        Message decoded = Message.fromBytes(message.toBytes());

        assertEquals(MessageBodyCase.STRUCTURED, decoded.getBodyCase());
        JsonObject body = (JsonObject) decoded.getBody();
        assertEquals(21.5, body.get("temperature").getAsDouble());
        assertTrue(body.get("ok").getAsBoolean());
        assertEquals("A", body.getAsJsonObject("nested").get("line").getAsString());
        assertEquals("short", decoded.getTags().toDict().get("retention").getAsString());
        assertEquals(5, decoded.getTags().toDict().get("priority").getAsInt());
    }

    @Test
    void southboundSignalUpdatePreservesByteSample() throws Exception {
        byte[] bytes = new byte[] {0, 1, 2, (byte) 254, (byte) 255};
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "camera-1/roi-17/thumbnail");
        signal.addProperty("name", "Thumbnail");
        JsonObject address = new JsonObject();
        address.addProperty("ns", 2);
        address.addProperty("nodeId", "Line1.Thumbnail");
        signal.add("address", address);
        body.add("signal", signal);
        JsonArray samples = new JsonArray();
        JsonObject sample = new JsonObject();
        sample.add("value", binaryMarker(bytes));
        sample.addProperty("quality", "GOOD");
        sample.addProperty("sourceTs", "2026-07-06T17:59:59.900Z");
        sample.addProperty("serverTs", "2026-07-06T18:00:00Z");
        samples.add(sample);
        body.add("samples", samples);

        Message message = MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                .withTimestampMs(1783360800000L)
                .withUuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1155")
                .withPayload(body)
                .build();

        EdgeCommonsMessage proto = EdgeCommonsMessage.parseFrom(message.toBytes());

        assertEquals(EdgeCommonsMessage.BodyCase.SOUTHBOUND_SIGNAL_UPDATE, proto.getBodyCase());
        assertEquals(ByteString.copyFrom(bytes),
                proto.getSouthboundSignalUpdate().getSamples(0).getValue().getBytesValue());
        assertEquals("Line1.Thumbnail",
                proto.getSouthboundSignalUpdate().getSignal().getAddress()
                        .getMapValue().getFieldsOrThrow("nodeId").getStringValue());
        assertEquals(1783360799900L,
                proto.getSouthboundSignalUpdate().getSamples(0).getSourceTsMs());
        Message decoded = Message.fromBytes(proto.toByteArray());
        assertEquals("Line1.Thumbnail", ((JsonObject) decoded.getBody())
                .getAsJsonObject("signal").getAsJsonObject("address").get("nodeId").getAsString());
        JsonObject decodedSample = ((JsonObject) decoded.getBody())
                .getAsJsonArray("samples").get(0).getAsJsonObject();
        assertArrayEquals(bytes, decodeBinaryMarker(decodedSample.getAsJsonObject("value")));
        assertEquals(MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE, decoded.getBodyCase());
    }

    @Test
    void opaqueBodyRoundTripsWithContentTypeAndSchema() {
        byte[] jpegLike = new byte[] {(byte) 0xff, (byte) 0xd8, (byte) 0xff, (byte) 0xe0, 1, 2};
        MessageBodySchema schema = new MessageBodySchema(
                "FramePreview", "1.0", "image/jpeg", "s3://descriptors/app.desc", "sha256:test");
        JsonObject tags = new JsonObject();
        tags.addProperty("capture_mode", "preview");

        Message message = MessageBuilder.create("FramePreview", "1.0")
                .withTimestampMs(1783360800000L)
                .withUuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f1156")
                .withOpaquePayload(jpegLike, "image/jpeg")
                .withSchema(schema)
                .withTags(MessageTags.fromDict(tags))
                .build();

        Message decoded = Message.fromBytes(message.toBytes());

        assertEquals(MessageBodyCase.OPAQUE, decoded.getBodyCase());
        assertEquals("image/jpeg", decoded.getContentType());
        assertEquals("s3://descriptors/app.desc", decoded.getSchema().descriptorRef());
        assertArrayEquals(jpegLike, decoded.getOpaqueBody());
        assertEquals("preview", decoded.getTags().toDict().get("capture_mode").getAsString());

        JsonObject diagnostic = decoded.toDiagnosticJson();
        assertEquals("OPAQUE", diagnostic.get("body_case").getAsString());
        assertEquals(jpegLike.length, diagnostic.getAsJsonObject("body").get("length").getAsInt());
        assertFalse(diagnostic.getAsJsonObject("body").has("_edgecommonsBinary"));
    }

    @Test
    void bytePayloadDefaultsToOpaqueOctetStream() {
        byte[] payload = new byte[] {10, 20, 30};

        Message message = MessageBuilder.create("OpaqueDefault", "1.0")
                .withPayload(payload)
                .build();
        Message decoded = Message.fromBytes(message.toBytes());

        assertEquals(MessageBodyCase.OPAQUE, decoded.getBodyCase());
        assertEquals("application/octet-stream", decoded.getContentType());
        assertArrayEquals(payload, decoded.getOpaqueBody());
    }

    @Test
    void reservedNamesInferTypedBodies() throws Exception {
        JsonObject state = new JsonObject();
        state.addProperty("status", "RUNNING");
        state.addProperty("uptimeSecs", 42);
        Message stateMessage = MessageBuilder.create("state", "1.0")
                .withTimestampMs(1783360800000L)
                .withPayload(state)
                .build();
        EdgeCommonsMessage stateProto = EdgeCommonsMessage.parseFrom(stateMessage.toBytes());
        assertEquals(EdgeCommonsMessage.BodyCase.STATE_UPDATE, stateProto.getBodyCase());
        assertEquals(42, stateProto.getStateUpdate().getUptimeSecs());
        assertEquals(MessageBodyCase.STATE_UPDATE, Message.fromBytes(stateProto.toByteArray()).getBodyCase());

        JsonObject cfg = new JsonObject();
        JsonObject document = new JsonObject();
        document.addProperty("mode", "auto");
        cfg.add("config", document);
        EdgeCommonsMessage cfgProto = EdgeCommonsMessage.parseFrom(MessageBuilder.create("cfg", "1.0")
                .withPayload(cfg)
                .build()
                .toBytes());
        assertEquals(EdgeCommonsMessage.BodyCase.CONFIG_UPDATE, cfgProto.getBodyCase());
        assertEquals("auto", cfgProto.getConfigUpdate().getConfig()
                .getMapValue().getFieldsOrThrow("mode").getStringValue());

        JsonObject metric = new JsonObject();
        metric.addProperty("namespace", "EdgeCommons");
        metric.addProperty("metricName", "MessagesPublished");
        JsonArray values = new JsonArray();
        JsonObject value = new JsonObject();
        value.addProperty("name", "Count");
        value.addProperty("value", 3.0);
        value.addProperty("unit", "Count");
        values.add(value);
        metric.add("values", values);
        EdgeCommonsMessage metricProto = EdgeCommonsMessage.parseFrom(MessageBuilder.create("Metric", "1.0")
                .withPayload(metric)
                .build()
                .toBytes());
        assertEquals(EdgeCommonsMessage.BodyCase.METRIC_UPDATE, metricProto.getBodyCase());
        assertEquals("MessagesPublished", metricProto.getMetricUpdate().getMetricName());

        JsonObject event = new JsonObject();
        event.addProperty("severity", "info");
        event.addProperty("type", "door-open");
        event.addProperty("message", "door opened");
        EdgeCommonsMessage eventProto = EdgeCommonsMessage.parseFrom(MessageBuilder.create("evt", "1.0")
                .withPayload(event)
                .build()
                .toBytes());
        assertEquals(EdgeCommonsMessage.BodyCase.EVENT, eventProto.getBodyCase());
        assertEquals("door-open", eventProto.getEvent().getType());
    }

    @Test
    void explicitCommandBodyPreservesComponentFacingPayload() throws Exception {
        JsonObject payload = new JsonObject();
        payload.addProperty("status", "RUNNING");

        Message message = MessageBuilder.create("ping", "1.0")
                .withCommand(payload)
                .build();
        EdgeCommonsMessage proto = EdgeCommonsMessage.parseFrom(message.toBytes());

        assertEquals(EdgeCommonsMessage.BodyCase.COMMAND, proto.getBodyCase());
        assertEquals("ping", proto.getCommand().getVerb());
        assertEquals("RUNNING", proto.getCommand().getPayload()
                .getMapValue().getFieldsOrThrow("status").getStringValue());
        Message decoded = Message.fromBytes(proto.toByteArray());
        assertEquals(MessageBodyCase.COMMAND, decoded.getBodyCase());
        assertEquals("RUNNING", ((JsonObject) decoded.getBody()).get("status").getAsString());
    }

    @Test
    void diagnosticJsonHeaderCarriesTimestampMillisAndCanBeReadBack() {
        JsonObject payload = new JsonObject();
        payload.addProperty("value", "ok");

        Message message = MessageBuilder.create("StructuredSample", "1.0")
                .withTimestampMs(1783360800000L)
                .withStructuredPayload(payload)
                .build();

        JsonObject diagnostic = message.toDiagnosticJson();
        assertEquals(1783360800000L,
                diagnostic.getAsJsonObject("header").get("timestamp_ms").getAsLong());

        Message parsed = MessageBuilder.fromObject(diagnostic);
        assertEquals(1783360800000L, parsed.getHeader().getTimestampMs());
    }

    @Test
    void descriptorSetIsGeneratedAtVectorPath() {
        Path descriptor = Path.of("../../protobuf-test-vectors/edgecommons-v1.desc").normalize();

        assertTrue(Files.isRegularFile(descriptor), "protobuf descriptor set must be generated");
    }

    @Test
    void protobufVectorsMatchJavaCanonicalBytes() throws Exception {
        Map<String, Message> messages = canonicalVectorMessages();
        Map<String, String> expectedHex = loadVectorHexes();

        assertEquals(messages.keySet(), expectedHex.keySet(), "vector ids must match canonical builders");
        for (Map.Entry<String, Message> entry : messages.entrySet()) {
            String id = entry.getKey();
            byte[] actual = entry.getValue().toBytes();
            assertEquals(expectedHex.get(id), toHex(actual), "exact protobuf bytes for " + id);
            Message decoded = Message.fromBytes(fromHex(expectedHex.get(id)));
            assertEquals(entry.getValue().getHeader().getName(), decoded.getHeader().getName());
            assertEquals(entry.getValue().getBodyCase(), decoded.getBodyCase());
        }

        Path manifest = Path.of("../../protobuf-test-vectors/messages.json").normalize();
        Path failures = Path.of("../../protobuf-test-vectors/failures.json").normalize();
        assertTrue(Files.isRegularFile(manifest), "messages.json vector manifest must exist");
        assertTrue(Files.isRegularFile(failures), "failures.json vector manifest must exist");
        assertEquals(messages.size(), JsonParser.parseString(Files.readString(manifest))
                .getAsJsonObject().getAsJsonArray("messages").size());
        assertTrue(JsonParser.parseString(Files.readString(failures))
                .getAsJsonObject().getAsJsonArray("cases").size() >= 3);
    }

    @Test
    void southboundSampleSnakeCaseAliasesAndRawQualityAreParsed() throws Exception {
        // The wire parser accepts both camelCase and snake_case sample keys. A snake_case-only
        // sample must land in the same typed proto fields as its camelCase twin.
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "sig-1");
        body.add("signal", signal);
        JsonArray samples = new JsonArray();
        JsonObject sample = new JsonObject();
        sample.addProperty("value", 7);
        sample.add("quality_raw", binaryMarker(new byte[] {9, 8}));
        sample.addProperty("source_ts", "2026-07-06T17:59:59.900Z");
        sample.addProperty("source_ts_ms", 111L);
        sample.addProperty("server_ts", "2026-07-06T18:00:00Z");
        sample.addProperty("server_ts_ms", 222L);
        samples.add(sample);
        body.add("samples", samples);

        EdgeCommonsMessage proto = EdgeCommonsMessage.parseFrom(MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                .withSouthboundSignalUpdate(body).build().toBytes());

        assertEquals("2026-07-06T17:59:59.900Z",
                proto.getSouthboundSignalUpdate().getSamples(0).getSourceTs(),
                "the snake_case source_ts alias maps to the sourceTs field");
        assertEquals(111L, proto.getSouthboundSignalUpdate().getSamples(0).getSourceTsMs(),
                "source_ts_ms alias maps to sourceTsMs");
        assertEquals("2026-07-06T18:00:00Z",
                proto.getSouthboundSignalUpdate().getSamples(0).getServerTs(),
                "server_ts alias maps to serverTs");
        assertEquals(222L, proto.getSouthboundSignalUpdate().getSamples(0).getServerTsMs(),
                "server_ts_ms alias maps to serverTsMs");
        assertArrayEquals(new byte[] {9, 8},
                proto.getSouthboundSignalUpdate().getSamples(0).getQualityRaw().getBytesValue().toByteArray(),
                "the quality_raw alias maps to the qualityRaw field");
    }

    @Test
    void unparseableSourceTimestampIsPreservedAndUnknownKeysSurvive() {
        // A non-ISO sourceTs is kept verbatim (no derived epoch millis), and any unknown key on a
        // sample or on the telemetry body round-trips through the proto extra map.
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "sig-2");
        body.add("signal", signal);
        JsonArray samples = new JsonArray();
        JsonObject sample = new JsonObject();
        sample.addProperty("value", 1);
        sample.addProperty("sourceTs", "not-a-timestamp");
        sample.addProperty("custom_tag", "keep-me");
        samples.add(sample);
        body.add("samples", samples);
        body.addProperty("batch", "b-9");

        Message decoded = Message.fromBytes(MessageBuilder.create("SouthboundSignalUpdate", "1.0")
                .withSouthboundSignalUpdate(body).build().toBytes());
        JsonObject decodedBody = (JsonObject) decoded.getBody();
        JsonObject decodedSample = decodedBody.getAsJsonArray("samples").get(0).getAsJsonObject();

        assertEquals("not-a-timestamp", decodedSample.get("sourceTs").getAsString(),
                "a non-ISO sourceTs is preserved verbatim");
        assertFalse(decodedSample.has("sourceTsMs"),
                "an unparseable sourceTs must not derive a sourceTsMs");
        assertEquals("keep-me", decodedSample.get("custom_tag").getAsString(),
                "an unknown sample key round-trips through the extra map");
        assertEquals("b-9", decodedBody.get("batch").getAsString(),
                "an unknown telemetry-body key round-trips through the extra map");
    }

    @Test
    void stateSnakeCaseUptimeAndInstanceDetailRoundTripAndNonObjectInstancesAreSkipped() {
        JsonObject state = new JsonObject();
        state.addProperty("status", "RUNNING");
        state.addProperty("uptime_secs", 99L);
        JsonArray instances = new JsonArray();
        instances.add("not-an-object");                 // skipped by the parser
        JsonObject inst = new JsonObject();
        inst.addProperty("instance", "kep1");
        inst.addProperty("connected", true);
        inst.addProperty("detail", "session ok");
        instances.add(inst);
        state.add("instances", instances);

        Message decoded = Message.fromBytes(MessageBuilder.create("state", "1.0")
                .withStateUpdate(state).build().toBytes());
        JsonObject body = (JsonObject) decoded.getBody();

        assertEquals(99L, body.get("uptimeSecs").getAsLong(),
                "the snake_case uptime_secs alias maps to uptimeSecs");
        JsonArray decodedInstances = body.getAsJsonArray("instances");
        assertEquals(1, decodedInstances.size(),
                "a non-object entry in instances[] is skipped, not encoded");
        assertEquals("session ok",
                decodedInstances.get(0).getAsJsonObject().get("detail").getAsString(),
                "the instance connectivity detail round-trips");
    }

    @Test
    void configUpdateWithoutConfigWrapperTreatsWholeBodyAsConfig() {
        JsonObject body = new JsonObject();
        body.addProperty("mode", "auto");
        body.addProperty("rateMs", 500);

        Message decoded = Message.fromBytes(MessageBuilder.create("cfg", "1.0")
                .withConfigUpdate(body).build().toBytes());
        JsonObject decodedBody = (JsonObject) decoded.getBody();

        assertEquals("auto", decodedBody.getAsJsonObject("config").get("mode").getAsString(),
                "a config body with no 'config' key is wrapped whole into the config field");
        assertEquals(500, decodedBody.getAsJsonObject("config").get("rateMs").getAsInt());
    }

    @Test
    void metricLargeFleetWorkaroundAndEmfProjectionRoundTrip() {
        JsonObject metric = new JsonObject();
        metric.addProperty("namespace", "EdgeCommons");
        metric.addProperty("metricName", "Published");
        metric.addProperty("largeFleetWorkaround", true);
        JsonObject emf = new JsonObject();
        emf.addProperty("_aws", "projected");
        metric.add("emfProjection", emf);
        JsonArray values = new JsonArray();
        JsonObject v = new JsonObject();
        v.addProperty("name", "Count");
        v.addProperty("value", 2.0);
        values.add(v);
        metric.add("values", values);

        Message decoded = Message.fromBytes(MessageBuilder.create("Metric", "1.0")
                .withMetricUpdate(metric).build().toBytes());
        JsonObject body = (JsonObject) decoded.getBody();

        assertTrue(body.get("largeFleetWorkaround").getAsBoolean(),
                "the large-fleet workaround flag round-trips");
        assertEquals("projected", body.getAsJsonObject("emfProjection").get("_aws").getAsString(),
                "the EMF projection object round-trips");
    }

    @Test
    void metricSnakeCaseAliasesAreParsedAndNonObjectValuesSkipped() throws Exception {
        JsonObject metric = new JsonObject();
        metric.addProperty("namespace", "EdgeCommons");
        metric.addProperty("metric_name", "Latency");
        metric.addProperty("timestamp_ms", 1783360800000L);
        metric.addProperty("large_fleet_workaround", true);
        JsonObject emf = new JsonObject();
        emf.addProperty("_aws", "x");
        metric.add("emf_projection", emf);
        JsonArray values = new JsonArray();
        values.add("not-an-object");                    // skipped
        JsonObject v = new JsonObject();
        v.addProperty("name", "p99");
        v.addProperty("value", 12.0);
        v.addProperty("unit", "Milliseconds");
        v.addProperty("storage_resolution", 1);
        values.add(v);
        metric.add("values", values);

        EdgeCommonsMessage proto = EdgeCommonsMessage.parseFrom(MessageBuilder.create("Metric", "1.0")
                .withMetricUpdate(metric).build().toBytes());

        assertEquals("Latency", proto.getMetricUpdate().getMetricName(),
                "the metric_name alias maps to metricName");
        assertEquals(1783360800000L, proto.getMetricUpdate().getTimestampMs(),
                "the timestamp_ms alias maps to timestampMs");
        assertTrue(proto.getMetricUpdate().getLargeFleetWorkaround(),
                "the large_fleet_workaround alias maps through");
        assertTrue(proto.getMetricUpdate().hasEmfProjection(),
                "the emf_projection alias maps to emfProjection");
        assertEquals(1, proto.getMetricUpdate().getValuesCount(),
                "a non-object metric value is skipped");
        assertEquals(1, proto.getMetricUpdate().getValues(0).getStorageResolution(),
                "the storage_resolution alias maps to storageResolution");
    }

    @Test
    void eventSnakeCaseTimestampAndAlarmActiveFlagsRoundTrip() {
        JsonObject event = new JsonObject();
        event.addProperty("severity", "warn");
        event.addProperty("type", "door");
        event.addProperty("message", "ajar");
        event.addProperty("timestamp_ms", 1783360800000L);
        event.addProperty("alarm", true);
        event.addProperty("active", false);

        Message decoded = Message.fromBytes(MessageBuilder.create("evt", "1.0")
                .withEvent(event).build().toBytes());
        JsonObject body = (JsonObject) decoded.getBody();

        assertEquals(1783360800000L, body.get("timestampMs").getAsLong(),
                "the snake_case timestamp_ms alias maps to timestampMs");
        assertTrue(body.get("alarm").getAsBoolean(), "the alarm flag round-trips");
        assertFalse(body.get("active").getAsBoolean(),
                "an explicit active=false is preserved (proto3 optional), not dropped");
    }

    @Test
    void commandWithExplicitPayloadAndStatusFlagsRoundTrip() {
        JsonObject command = new JsonObject();
        command.addProperty("verb", "restart");
        JsonObject payload = new JsonObject();
        payload.addProperty("target", "svc-1");
        command.add("payload", payload);
        command.addProperty("ok", true);

        Message decoded = Message.fromBytes(MessageBuilder.create("restart", "1.0")
                .withCommand(command).build().toBytes());
        JsonObject body = (JsonObject) decoded.getBody();

        assertEquals("restart", body.get("verb").getAsString());
        assertEquals("svc-1", body.getAsJsonObject("payload").get("target").getAsString(),
                "an explicit command payload round-trips alongside status fields");
        assertTrue(body.get("ok").getAsBoolean());
    }

    @Test
    void commandErrorWithDetailsRoundTrips() {
        JsonObject command = new JsonObject();
        JsonObject error = new JsonObject();
        error.addProperty("code", "E_TIMEOUT");
        error.addProperty("message", "no reply");
        JsonObject details = new JsonObject();
        details.addProperty("waitedMs", 30000);
        error.add("details", details);
        command.add("error", error);

        Message decoded = Message.fromBytes(MessageBuilder.create("setState.reply", "1.0")
                .withCommand(command).build().toBytes());
        JsonObject decodedError = ((JsonObject) decoded.getBody()).getAsJsonObject("error");

        assertEquals("E_TIMEOUT", decodedError.get("code").getAsString());
        assertEquals("no reply", decodedError.get("message").getAsString());
        assertEquals(30000, decodedError.getAsJsonObject("details").get("waitedMs").getAsInt(),
                "structured command error details survive the round trip");
    }

    @Test
    void structuredBodyWithNullsAndArraysRoundTrips() {
        JsonObject body = new JsonObject();
        body.add("maybe", JsonNull.INSTANCE);
        JsonArray arr = new JsonArray();
        arr.add(1);
        arr.add("two");
        arr.add(true);
        body.add("readings", arr);

        Message decoded = Message.fromBytes(MessageBuilder.create("Sample", "1.0")
                .withStructuredBody(body).build().toBytes());
        JsonObject decodedBody = (JsonObject) decoded.getBody();

        assertTrue(decodedBody.get("maybe").isJsonNull(),
                "a null structured value round-trips as JSON null");
        JsonArray readings = decodedBody.getAsJsonArray("readings");
        assertEquals(1, readings.get(0).getAsInt());
        assertEquals("two", readings.get(1).getAsString());
        assertTrue(readings.get(2).getAsBoolean());
    }

    @Test
    void contentEncodingRoundTrips() {
        JsonObject body = new JsonObject();
        body.addProperty("x", 1);

        Message decoded = Message.fromBytes(MessageBuilder.create("Sample", "1.0")
                .withStructuredBody(body).withContentEncoding("gzip").build().toBytes());

        assertEquals("gzip", decoded.getContentEncoding(),
                "the content encoding survives the protobuf round trip");
    }

    @Test
    void bodilessMessageInferredAsBodyNotSetAndRoundTrips() {
        Message message = MessageBuilder.create("NoBody", "1.0").build();
        assertEquals(MessageBodyCase.BODY_NOT_SET, message.getBodyCase(),
                "a message with no payload infers BODY_NOT_SET");

        Message decoded = Message.fromBytes(message.toBytes());
        assertEquals(MessageBodyCase.BODY_NOT_SET, decoded.getBodyCase(),
                "an absent body round-trips as BODY_NOT_SET");
        assertNull(decoded.getBody(), "no body is materialized for a bodiless message");
    }

    @Test
    void inboundBinaryMarkerObjectInfersOpaqueBodyCase() {
        JsonObject markerBody = binaryMarker(new byte[] {5, 6, 7});
        Message message = MessageBuilder.create("Frame", "1.0").withPayload(markerBody).build();

        assertEquals(MessageBodyCase.OPAQUE, MessageProtoCodec.bodyCase(message),
                "a body carrying the binary marker is inferred OPAQUE");
    }

    @Test
    void parsingProtoWithoutHeaderIsRejected() {
        byte[] headerless = EdgeCommonsMessage.newBuilder().build().toByteArray();

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> Message.fromBytes(headerless));
        assertTrue(ex.getMessage().contains("header name and version"),
                "a protobuf message with no header is rejected with guidance");
    }

    @Test
    void parsingProtoWithMalformedIdentityIsRejected() {
        byte[] bytes = EdgeCommonsMessage.newBuilder()
                .setHeader(Header.newBuilder().setName("Evt").setVersion("1.0"))
                .setIdentity(Identity.newBuilder().setComponent("comp"))  // no hier -> malformed
                .build()
                .toByteArray();

        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> Message.fromBytes(bytes));
        assertTrue(ex.getMessage().contains("Malformed protobuf identity"),
                "an identity with no hierarchy is rejected");
    }

    @Test
    void binaryMarkerRejectsNonBase64EncodingAndLengthMismatch() {
        JsonObject wrongEncoding = new JsonObject();
        JsonObject d1 = new JsonObject();
        d1.addProperty("encoding", "hex");
        d1.addProperty("length", 1);
        d1.addProperty("data", "AQ==");
        wrongEncoding.add("_edgecommonsBinary", d1);
        Message m1 = MessageBuilder.create("Sample", "1.0").withStructuredBody(wrongEncoding).build();
        IllegalArgumentException e1 = assertThrows(IllegalArgumentException.class, m1::toBytes);
        assertTrue(e1.getMessage().contains("base64"),
                "a non-base64 binary-marker encoding is rejected");

        JsonObject wrongLength = new JsonObject();
        JsonObject d2 = new JsonObject();
        d2.addProperty("encoding", "base64");
        d2.addProperty("length", 99);
        d2.addProperty("data", Base64.getEncoder().encodeToString(new byte[] {1, 2}));
        wrongLength.add("_edgecommonsBinary", d2);
        Message m2 = MessageBuilder.create("Sample", "1.0").withStructuredBody(wrongLength).build();
        IllegalArgumentException e2 = assertThrows(IllegalArgumentException.class, m2::toBytes);
        assertTrue(e2.getMessage().contains("length"),
                "a binary marker whose declared length disagrees with the data is rejected");
    }

    private static JsonObject binaryMarker(byte[] bytes) {
        JsonObject descriptor = new JsonObject();
        descriptor.addProperty("encoding", "base64");
        descriptor.addProperty("length", bytes.length);
        descriptor.addProperty("data", Base64.getEncoder().encodeToString(bytes));
        JsonObject marker = new JsonObject();
        marker.add("_edgecommonsBinary", descriptor);
        return marker;
    }

    private static byte[] decodeBinaryMarker(JsonObject marker) {
        JsonObject descriptor = marker.getAsJsonObject("_edgecommonsBinary");
        return Base64.getDecoder().decode(descriptor.get("data").getAsString());
    }

    private static LinkedHashMap<String, Message> canonicalVectorMessages() {
        MessageIdentity identity = new MessageIdentity(List.of(
                new MessageIdentity.HierEntry("site", "plant-a"),
                new MessageIdentity.HierEntry("line", "line-2"),
                new MessageIdentity.HierEntry("device", "gw-01")
        ), "interop", "main");

        JsonObject siteRoleTags = new JsonObject();
        siteRoleTags.addProperty("siteRole", "line-edge");
        JsonObject captureTags = new JsonObject();
        captureTags.addProperty("capture_mode", "preview");
        JsonObject relayTags = new JsonObject();
        relayTags.addProperty("_relay", "uns-bridge:a:1");
        relayTags.addProperty("priority", 5);

        byte[] opaqueBytes = new byte[] {(byte) 0xff, (byte) 0xd8, (byte) 0xff, (byte) 0xe0, 1, 2};
        MessageBodySchema opaqueSchema = new MessageBodySchema(
                "FramePreview", "1.0", "image/jpeg",
                "s3://edgecommons-descriptors/edgecommons-v1.desc", "sha256:test");

        LinkedHashMap<String, Message> messages = new LinkedHashMap<>();
        messages.put("telemetry_numeric", vectorBase("Telemetry", "1001", "corr-vector-telemetry_numeric")
                .withPayload(telemetryNumericBody())
                .withIdentity(identity)
                .withTags(MessageTags.fromDict(siteRoleTags))
                .build());
        messages.put("telemetry_byte_timestamps",
                vectorBase("SouthboundSignalUpdate", "1002", "corr-vector-telemetry_byte_timestamps")
                        .withSouthboundSignalUpdate(telemetryByteBody())
                        .withIdentity(identity)
                        .build());
        messages.put("opaque_jpeg", vectorBase("FramePreview", "1003", "corr-vector-opaque_jpeg")
                .withOpaqueBody(opaqueBytes, "image/jpeg")
                .withSchema(opaqueSchema)
                .withTags(MessageTags.fromDict(captureTags))
                .build());
        messages.put("tagged_relay_envelope",
                vectorBase("Telemetry", "1004", "corr-vector-tagged_relay_envelope")
                        .withPayload(telemetryNumericBody())
                        .withIdentity(identity)
                        .withTags(MessageTags.fromDict(relayTags))
                        .build());

        JsonObject commandPayload = new JsonObject();
        commandPayload.addProperty("status", "RUNNING");
        messages.put("command_request", vectorBase("setState", "1005", "corr-command-1")
                .withReplyTo("reply/interop/setState")
                .withCommand(commandPayload)
                .build());

        JsonObject commandReply = new JsonObject();
        commandReply.addProperty("ok", true);
        JsonObject result = new JsonObject();
        result.addProperty("accepted", true);
        commandReply.add("result", result);
        messages.put("command_reply", vectorBase("setState.reply", "1006", "corr-command-1")
                .withCommand(commandReply)
                .build());

        JsonObject state = new JsonObject();
        state.addProperty("status", "RUNNING");
        state.addProperty("uptimeSecs", 42);
        JsonArray instances = new JsonArray();
        JsonObject instance = new JsonObject();
        instance.addProperty("instance", "main");
        instance.addProperty("connected", true);
        instances.add(instance);
        state.add("instances", instances);
        messages.put("state_reserved", vectorBase("state", "1007", "corr-vector-state_reserved")
                .withPayload(state)
                .build());

        JsonObject config = new JsonObject();
        JsonObject configDocument = new JsonObject();
        configDocument.addProperty("mode", "auto");
        configDocument.addProperty("sampleRateMs", 1000);
        config.add("config", configDocument);
        messages.put("config_update", vectorBase("ConfigUpdate", "1008", "corr-vector-config_update")
                .withConfigUpdate(config)
                .build());

        JsonObject metric = new JsonObject();
        metric.addProperty("namespace", "EdgeCommons");
        metric.addProperty("metricName", "MessagesPublished");
        metric.addProperty("timestampMs", 1783360800000L);
        JsonObject dimensions = new JsonObject();
        dimensions.addProperty("component", "interop");
        dimensions.addProperty("instance", "main");
        metric.add("dimensions", dimensions);
        JsonArray metricValues = new JsonArray();
        JsonObject metricValue = new JsonObject();
        metricValue.addProperty("name", "Count");
        metricValue.addProperty("value", 3.0);
        metricValue.addProperty("unit", "Count");
        metricValue.addProperty("storageResolution", 60);
        metricValues.add(metricValue);
        metric.add("values", metricValues);
        messages.put("metric_update", vectorBase("MetricUpdate", "1009", "corr-vector-metric_update")
                .withMetricUpdate(metric)
                .build());

        JsonObject event = new JsonObject();
        event.addProperty("severity", "info");
        event.addProperty("type", "door-open");
        event.addProperty("message", "door opened");
        event.addProperty("timestamp", "2026-07-06T18:00:00Z");
        event.addProperty("timestampMs", 1783360800000L);
        JsonObject context = new JsonObject();
        context.addProperty("door", "dock-7");
        context.addProperty("open", true);
        event.add("context", context);
        messages.put("event_message", vectorBase("Event", "1010", "corr-vector-event_message")
                .withEvent(event)
                .build());

        JsonObject structured = new JsonObject();
        structured.addProperty("temperature", 21.5);
        structured.addProperty("ok", true);
        JsonObject nested = new JsonObject();
        nested.addProperty("line", "A");
        structured.add("nested", nested);
        messages.put("structured_generic", vectorBase("StructuredSample", "1011", "corr-vector-structured_generic")
                .withStructuredBody(structured)
                .withTags(MessageTags.fromDict(siteRoleTags))
                .build());
        return messages;
    }

    private static MessageBuilder vectorBase(String name, String uuidSuffix, String correlationId) {
        return MessageBuilder.create(name, "1.0")
                .withTimestampMs(1783360800000L)
                .withUuid("018fe1dd-7dc7-7b0f-a80f-5d5d6d0f" + uuidSuffix)
                .withCorrelationId(correlationId);
    }

    private static JsonObject telemetryNumericBody() {
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "temp");
        signal.addProperty("name", "Temperature");
        JsonObject address = new JsonObject();
        address.addProperty("ns", 2);
        address.addProperty("nodeId", "Line1.Temp");
        signal.add("address", address);
        body.add("signal", signal);
        JsonArray samples = new JsonArray();
        JsonObject sample = new JsonObject();
        sample.addProperty("value", 21.5);
        sample.addProperty("quality", "GOOD");
        sample.addProperty("sourceTs", "2026-07-06T17:59:59.900Z");
        sample.addProperty("serverTs", "2026-07-06T18:00:00Z");
        samples.add(sample);
        body.add("samples", samples);
        return body;
    }

    private static JsonObject telemetryByteBody() {
        JsonObject body = new JsonObject();
        JsonObject signal = new JsonObject();
        signal.addProperty("id", "camera-1/roi-17/thumbnail");
        signal.addProperty("name", "Thumbnail");
        body.add("signal", signal);
        JsonArray samples = new JsonArray();
        JsonObject sample = new JsonObject();
        sample.add("value", binaryMarker(new byte[] {0, 1, 2, (byte) 254, (byte) 255}));
        sample.addProperty("quality", "GOOD");
        sample.addProperty("sourceTsMs", 1783360799900L);
        sample.addProperty("serverTsMs", 1783360800000L);
        samples.add(sample);
        body.add("samples", samples);
        return body;
    }

    private static Map<String, String> loadVectorHexes() throws Exception {
        LinkedHashMap<String, String> vectors = new LinkedHashMap<>();
        Path path = Path.of("../../protobuf-test-vectors/messages.pb.hex").normalize();
        for (String line : Files.readAllLines(path)) {
            if (line.isBlank() || line.startsWith("#")) {
                continue;
            }
            String[] parts = line.split(" ", 2);
            vectors.put(parts[0], parts[1]);
        }
        return vectors;
    }

    private static String toHex(byte[] bytes) {
        StringBuilder out = new StringBuilder(bytes.length * 2);
        for (byte b : bytes) {
            out.append(String.format("%02x", b & 0xff));
        }
        return out.toString();
    }

    private static byte[] fromHex(String hex) {
        byte[] bytes = new byte[hex.length() / 2];
        for (int i = 0; i < bytes.length; i++) {
            bytes[i] = (byte) Integer.parseInt(hex.substring(i * 2, i * 2 + 2), 16);
        }
        return bytes;
    }
}
