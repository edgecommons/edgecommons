/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config;

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
