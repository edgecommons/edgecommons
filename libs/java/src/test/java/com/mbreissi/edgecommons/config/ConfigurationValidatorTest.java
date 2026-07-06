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
    void messagingRequestTimeoutValidates() {
        // messaging.requestTimeoutSeconds remains a generic messaging config knob.
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "messaging":{"requestTimeoutSeconds":30}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));
    }

    @Test
    void messagingBrokerQosValidatesAndTopLevelQosIsRejected() {
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "messaging":{\
                "local":{"host":"localhost","port":1883,"clientId":"local",\
                "qos":{"publish":1,"subscribe":1}},\
                "northbound":{"host":"broker.example.com","port":8883,"clientId":"northbound",\
                "qos":{"publish":2,"subscribe":1}}}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));

        JsonObject staleTopLevelQos = obj("""
                {"component":{"global":{}},\
                "messaging":{"local":{"host":"localhost","port":1883,"clientId":"local"},\
                "qos":{"local":{"publish":1}}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleTopLevelQos));
    }

    @Test
    void genericMessagingLwtIsRejected() {
        JsonObject staleGenericLwt = obj("""
                {"component":{"global":{}},\
                "messaging":{"lwt":{"topic":"ecv1/gw-01/bridge/main/state",\
                "payload":{"status":"UNREACHABLE"},"qos":1}}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleGenericLwt));
    }

    @Test
    void heartbeatNewShapeValidatesAndDriftKnobsAreRejected() {
        // UNS slice 1d (§4.3, D-U14/D-U20): heartbeat = enabled/intervalSecs/measures/destination.
        JsonObject valid = obj("""
                {"component":{"global":{}},\
                "heartbeat":{"enabled":true,"intervalSecs":5,\
                "measures":{"cpu":true,"memory":true},"destination":"local"}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(valid));

        JsonObject northbound = obj("""
                {"component":{"global":{}},"heartbeat":{"destination":"northbound"}}""");
        assertDoesNotThrow(() -> ConfigurationValidator.validate(northbound));

        // The legacy targets[] array (the topic-override drift knobs) is REMOVED - a stale config
        // must fail with a precise error (§10 hard cut).
        JsonObject staleTargets = obj("""
                {"component":{"global":{}},\
                "heartbeat":{"intervalSecs":5,"targets":[{"type":"metric"}]}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleTargets));

        // destination is a closed enum: local | northbound (no legacy iotcore/iot_core aliases).
        JsonObject badDestination = obj("""
                {"component":{"global":{}},"heartbeat":{"destination":"iot_core"}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(badDestination));
        JsonObject staleIotcore = obj("""
                {"component":{"global":{}},"heartbeat":{"destination":"iotcore"}}""");
        assertThrows(ConfigurationValidator.ConfigurationValidationException.class,
                () -> ConfigurationValidator.validate(staleIotcore));
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
