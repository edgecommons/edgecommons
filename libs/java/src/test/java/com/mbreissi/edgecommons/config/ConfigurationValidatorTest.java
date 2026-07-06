/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link ConfigurationValidator} covering both the pass and fail
 * paths of JSON-schema validation (schema resource is bundled on the classpath).
 */
class ConfigurationValidatorTest {

    private static JsonObject obj(String json) {
        return JsonParser.parseString(json).getAsJsonObject();
    }

    @Test
    void validConfigurationPasses() {
        // "component" with "global" is the only required block per the schema.
        JsonObject valid = obj("""
                {"component":{"global":{"timeout":1000}},\
                "logging":{"level":"INFO"},\
                "metricEmission":{"target":"log"},\
                "heartbeat":{"intervalSecs":5},\
                "tags":{"env":"prod"}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));
    }

    @Test
    void missingRequiredComponentFails() {
        // "component" is required at the top level.
        JsonObject invalid = obj("""
                {"logging":{"level":"INFO"}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(invalid));
    }

    @Test
    void invalidEnumValueFails() {
        // "BOGUS" is not a valid logging level enum value.
        JsonObject invalid = obj("""
                {"component":{"global":{}},"logging":{"level":"BOGUS"}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(invalid));
    }

    @Test
    void additionalPropertyAtRootFails() {
        // additionalProperties:false at root rejects unknown keys.
        JsonObject invalid = obj("""
                {"component":{"global":{}},"unknownTopLevel":true}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(invalid));
    }

    @Test
    void parametersSectionPasses() {
        // The strict root schema (additionalProperties:false) must permit the "parameters" section
        // (subsystem owns its inner schema), exactly as it permits "credentials"/"streaming".
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "parameters":{"source":{"type":"env","prefix":"GG_PARAM_"},\
                "sync":{"names":["/myapp/region"]}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));
    }

    @Test
    void messagingRequestTimeoutAndLwtValidate() {
        // UNS slice 1a/1c: messaging.requestTimeoutSeconds (number, min 0) and messaging.lwt
        // (topic required; payload string|object; qos enum [0,1]) must pass schema validation.
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "messaging":{"requestTimeoutSeconds":30,\
                "lwt":{"topic":"ecv1/gw-01/bridge/main/state",\
                "payload":{"status":"UNREACHABLE"},"qos":1}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));

        JsonObject stringPayloadQos0 = obj("""
                {"component":{"global":{}},\
                "messaging":{"lwt":{"topic":"t","payload":"OFFLINE","qos":0}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(stringPayloadQos0));
    }

    @Test
    void lwtQosAsLosslessDoubleValidates() {
        // The flagged 1b case: the schema types qos as "number" with enum [0,1]; a source that
        // delivers 1.0 (e.g. a numeric round-trip through a double) must still validate, since
        // JSON-Schema numeric comparison is mathematical, not lexical.
        JsonObject qosDouble = obj("""
                {"component":{"global":{}},\
                "messaging":{"lwt":{"topic":"t","qos":1.0}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(qosDouble));
    }

    @Test
    void lwtRejectsBadQosMissingTopicAndRetain() {
        // qos outside the enum
        JsonObject badQos = obj("""
                {"component":{"global":{}},"messaging":{"lwt":{"topic":"t","qos":2}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(badQos));
        // topic is required
        JsonObject noTopic = obj("""
                {"component":{"global":{}},"messaging":{"lwt":{"payload":"x"}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(noTopic));
        // NO retain knob by design (additionalProperties:false inside lwt)
        JsonObject retain = obj("""
                {"component":{"global":{}},"messaging":{"lwt":{"topic":"t","retain":true}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(retain));
    }

    @Test
    void heartbeatNewShapeValidatesAndDriftKnobsAreRejected() {
        // UNS slice 1d (§4.3, D-U14/D-U20): heartbeat = enabled/intervalSecs/measures/destination.
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "heartbeat":{"enabled":true,"intervalSecs":5,\
                "measures":{"cpu":true,"memory":true},"destination":"local"}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));

        JsonObject iotcore = obj("""
                {"component":{"global":{}},"heartbeat":{"destination":"iotcore"}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(iotcore));

        // The legacy targets[] array (the topic-override drift knobs) is REMOVED - a stale config
        // must fail with a precise error (§10 hard cut).
        JsonObject staleTargets = obj("""
                {"component":{"global":{}},\
                "heartbeat":{"intervalSecs":5,"targets":[{"type":"metric"}]}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleTargets));

        // destination is a closed enum: local | iotcore (no legacy ipc/iot_core aliases).
        JsonObject badDestination = obj("""
                {"component":{"global":{}},"heartbeat":{"destination":"iot_core"}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(badDestination));
    }

    @Test
    void metricEmissionTopicOverrideIsRejected() {
        // UNS slice 1d (§4.3, D-U9): metricEmission.targetConfig.topic is REMOVED (the messaging
        // target publishes to the UNS metric topic); destination survives.
        JsonObject destinationOnly = obj("""
                {"component":{"global":{}},\
                "metricEmission":{"target":"messaging","targetConfig":{"destination":"ipc"}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(destinationOnly));

        JsonObject staleTopic = obj("""
                {"component":{"global":{}},\
                "metricEmission":{"target":"messaging",\
                "targetConfig":{"topic":"a/b/c","destination":"ipc"}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleTopic));
    }

    @Test
    void negativeRequestTimeoutIsRejected() {
        JsonObject negative = obj("""
                {"component":{"global":{}},"messaging":{"requestTimeoutSeconds":-1}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(negative));
    }

    @Test
    void validationExceptionConstructorsAreUsable() {
        ConfigurationValidator.ConfigurationValidationException e1 =
                new ConfigurationValidator.ConfigurationValidationException("msg");
        assertEquals("msg", e1.getMessage());

        Throwable cause = new IllegalStateException("root");
        ConfigurationValidator.ConfigurationValidationException e2 =
                new ConfigurationValidator.ConfigurationValidationException("msg2", cause);
        assertEquals("msg2", e2.getMessage());
        assertSame(cause, e2.getCause());
    }
}
