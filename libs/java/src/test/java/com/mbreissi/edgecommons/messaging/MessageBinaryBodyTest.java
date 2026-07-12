/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.mbreissi.edgecommons.messaging.proto.MessageBodyCase;
import com.mbreissi.edgecommons.messaging.proto.MessageBodySchema;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.Base64;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Covers the binary-body wire contract ({@code _edgecommonsBinary} marker), the descriptor
 * validation rules, and the typed-payload / content-metadata variants of {@link MessageBuilder}.
 */
class MessageBinaryBodyTest {

    private static final String MARKER = "_edgecommonsBinary";

    private static Message message(Object body) {
        return MessageBuilder.create("Test", "1.0").withPayload(body).build();
    }

    /** Builds a message whose body is a raw binary marker object (as it arrives off the wire). */
    private static Message fromMarker(JsonObject descriptor) {
        JsonObject body = new JsonObject();
        body.add(MARKER, descriptor);
        return MessageBuilder.create("Test", "1.0").withStructuredPayload(body).build();
    }

    private static JsonObject descriptor(String encoding, Integer length, String data) {
        JsonObject d = new JsonObject();
        if (encoding != null) d.addProperty("encoding", encoding);
        if (length != null) d.addProperty("length", length);
        if (data != null) d.addProperty("data", data);
        return d;
    }

    // ------------------------------------------------------------------ binary bodies

    @Test
    void binaryBodyRoundTripsThroughTheMarkerEnvelope() {
        byte[] payload = "hello bytes".getBytes(StandardCharsets.UTF_8);
        Message msg = MessageBuilder.create("Test", "1.0").withOpaquePayload(payload).build();

        assertTrue(msg.isBinaryBody());
        assertEquals(MessageBodyCase.OPAQUE, msg.getBodyCase());
        assertEquals("application/octet-stream", msg.getContentType());
        assertArrayEquals(payload, msg.getBinaryBody());
        assertArrayEquals(payload, msg.getOpaqueBody());

        JsonObject dict = msg.toDict();
        JsonObject marker = dict.getAsJsonObject("body").getAsJsonObject(MARKER);
        assertEquals("base64", marker.get("encoding").getAsString());
        assertEquals(payload.length, marker.get("length").getAsInt());
        assertArrayEquals(payload, Base64.getDecoder().decode(marker.get("data").getAsString()));
    }

    @Test
    void getBinaryBodyReturnsACopy() {
        byte[] payload = {1, 2, 3};
        Message msg = MessageBuilder.create("Test", "1.0").withOpaqueBody(payload).build();

        byte[] first = msg.getBinaryBody();
        first[0] = 9;

        assertArrayEquals(payload, msg.getBinaryBody());
    }

    @Test
    void oversizedBinaryBodyIsRejected() {
        byte[] tooBig = new byte[Message.MAX_BINARY_BODY_BYTES + 1];
        Message msg = message(tooBig);

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("exceeds"), error.getMessage());
    }

    @Test
    void nonBinaryBodyHasNoBinaryOrOpaqueView() {
        Message msg = MessageBuilder.create("Test", "1.0")
                .withStructuredPayload(JsonParser.parseString("{\"a\":1}").getAsJsonObject())
                .build();

        assertNull(msg.getBinaryBody());
        assertNull(msg.getOpaqueBody());
        assertEquals(MessageBodyCase.STRUCTURED, msg.getBodyCase());
    }

    @Test
    void decodesAWellFormedMarkerFromTheWire() {
        byte[] payload = {10, 20, 30};
        Message msg = fromMarker(descriptor("base64", payload.length, Base64.getEncoder().encodeToString(payload)));

        assertArrayEquals(payload, msg.getBinaryBody());
    }

    @Test
    void markerMustBeAnObject() {
        JsonObject body = new JsonObject();
        body.addProperty(MARKER, "not-an-object");
        Message msg = MessageBuilder.create("Test", "1.0").withStructuredPayload(body).build();

        assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
    }

    @Test
    void markerEncodingMustBeBase64() {
        Message msg = fromMarker(descriptor("hex", 1, "AQ=="));

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("base64"), error.getMessage());
    }

    @Test
    void markerLengthIsRequired() {
        Message msg = fromMarker(descriptor("base64", null, "AQ=="));

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("length is required"), error.getMessage());
    }

    @Test
    void markerLengthMustBeWithinTheCap() {
        Message msg = fromMarker(descriptor("base64", Message.MAX_BINARY_BODY_BYTES + 1, "AQ=="));

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("exceeds"), error.getMessage());
    }

    @Test
    void markerDataIsRequired() {
        Message msg = fromMarker(descriptor("base64", 1, null));

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("data is required"), error.getMessage());
    }

    @Test
    void markerLengthMustMatchTheDecodedData() {
        Message msg = fromMarker(descriptor("base64", 99, Base64.getEncoder().encodeToString(new byte[]{1, 2})));

        IllegalArgumentException error = assertThrows(IllegalArgumentException.class, msg::getBinaryBody);
        assertTrue(error.getMessage().contains("does not match"), error.getMessage());
    }

    @Test
    void binaryBodyMarkerHelperRejectsOversizedInput() {
        assertThrows(IllegalArgumentException.class,
                () -> Message.binaryBodyMarker(new byte[Message.MAX_BINARY_BODY_BYTES + 1]));
    }

    // ------------------------------------------------------------------ builder variants

    @Test
    void typedPayloadVariantsSetTheirBodyCase() {
        JsonObject payload = JsonParser.parseString("{\"v\":1}").getAsJsonObject();

        assertEquals(MessageBodyCase.SOUTHBOUND_SIGNAL_UPDATE,
                MessageBuilder.create("T", "1").withSouthboundSignalUpdate(payload).build().getBodyCase());
        assertEquals(MessageBodyCase.STATE_UPDATE,
                MessageBuilder.create("T", "1").withStateUpdate(payload).build().getBodyCase());
        assertEquals(MessageBodyCase.CONFIG_UPDATE,
                MessageBuilder.create("T", "1").withConfigUpdate(payload).build().getBodyCase());
    }

    @Test
    void opaqueVariantsShareOneImplementation() {
        byte[] payload = {7, 7, 7};

        Message explicit = MessageBuilder.create("T", "1").withOpaqueBody(payload, "image/jpeg").build();
        assertEquals("image/jpeg", explicit.getContentType());
        assertArrayEquals(payload, explicit.getOpaqueBody());

        Message defaulted = MessageBuilder.create("T", "1").withOpaquePayload(payload, null).build();
        assertEquals("application/octet-stream", defaulted.getContentType());

        Message empty = MessageBuilder.create("T", "1").withOpaquePayload(null).build();
        assertNull(empty.getBody());
    }

    @Test
    void contentMetadataIsCarriedOnTheEnvelope() {
        MessageBodySchema schema = new MessageBodySchema("Signal", "1.0", "application/json", "ref", "sha256:abc");
        Message msg = MessageBuilder.create("T", "1")
                .withStructuredPayload(JsonParser.parseString("{\"a\":1}").getAsJsonObject())
                .withContentType("application/json")
                .withContentEncoding("gzip")
                .withSchema(schema)
                .build();

        assertEquals("application/json", msg.getContentType());
        assertEquals("gzip", msg.getContentEncoding());
        assertEquals(schema, msg.getSchema());

        JsonObject dict = msg.toDict();
        assertEquals("application/json", dict.get("content_type").getAsString());
        assertEquals("gzip", dict.get("content_encoding").getAsString());
        assertEquals("Signal", dict.getAsJsonObject("schema").get("name").getAsString());
    }

    @Test
    void fromObjectRestoresContentMetadata() {
        MessageBodySchema schema = new MessageBodySchema("Signal", "1.0", "application/json", null, null);
        Message original = MessageBuilder.create("T", "1")
                .withStructuredPayload(JsonParser.parseString("{\"a\":1}").getAsJsonObject())
                .withContentType("application/json")
                .withContentEncoding("gzip")
                .withSchema(schema)
                .build();

        Message restored = MessageBuilder.fromObject(original.toDict());

        assertEquals("application/json", restored.getContentType());
        assertEquals("gzip", restored.getContentEncoding());
        assertEquals(schema, restored.getSchema());
    }
}
