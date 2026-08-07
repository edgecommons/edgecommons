/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonElement;
import com.google.gson.JsonNull;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.google.gson.JsonPrimitive;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * The normative numeric-configuration contract (D-NC1 / D-NC2), pinned in the canonical language.
 *
 * <p><b>Assertions are on serialized text, deliberately.</b> Gson's {@code JsonPrimitive.equals}
 * compares numbers by their {@code double} value, so {@code 5000} and {@code 5000.0} compare
 * <em>equal</em> as {@link JsonElement}s. The whole point of this pass is the difference between
 * those two documents, so every expectation below is written against {@code toString()} — the
 * bytes a component, a candidate validator, and the published {@code cfg} document actually see.
 */
class ConfigNumbersTest {

    /** One row of the normative behavior table: JSON literal in, JSON literal out. */
    private record Row(String input, String expected, String rationale) { }

    /** Canonicalizes a JSON document and returns its exact serialized form. */
    private static String canon(String json) {
        return ConfigNumbers.canonicalizeNumbers(JsonParser.parseString(json)).toString();
    }

    // ------------------------------------------------------------------
    // The normative behavior table (DESIGN-config-numeric-canonicalization §3)
    // ------------------------------------------------------------------

    @Test
    void behaviorTable() {
        Row[] rows = {
            new Row("5000", "5000", "integer input is already canonical"),
            new Row("5000.0", "5000", "THE DEFECT: an integral double becomes an integer"),
            new Row("5000.00", "5000", "exactness is decided on the decimal, not on a double"),
            new Row("5e3", "5000", "exponent notation with an integral value converts"),
            new Row("5000.5", "5000.5", "fractional is left byte-identical, never truncated"),
            new Row("-5", "-5", "negative integer input is already canonical"),
            new Row("-5.0", "-5", "an integral negative double becomes an integer"),
            new Row("-0.0", "0", "IEEE -0.0 == 0.0, so the round-trip holds"),
            new Row("0.1", "0.1", "a fractional decimal survives verbatim"),
            new Row("1e19", "10000000000000000000", "integral and inside the unsigned 64-bit window"),
            new Row("1e20", "1e20", "integral but beyond 2^64-1 - untouched"),
            new Row("9223372036854775808.0", "9223372036854775808",
                    "2^63 lands in the unsigned half of the window"),
            new Row("-9223372036854775808.0", "-9223372036854775808",
                    "-2^63 is the lower bound of the window"),
            new Row("-9223372036854775809.0", "-9223372036854775809.0",
                    "below -2^63 - untouched"),
            new Row("9007199254740992.0", "9007199254740992", "the 2^53 boundary converts"),
            new Row("9007199254740993", "9007199254740993",
                    "an odd integer past 2^53 can only arrive as an integer literal, and survives"),
            new Row("18446744073709549568.0", "18446744073709549568",
                    "2^64-2048: the largest window value a double represents exactly"),
            new Row("18446744073709551615.0", "18446744073709551615", "2^64-1 itself converts"),
            new Row("18446744073709551616.0", "18446744073709551616.0",
                    "2^64 is outside the window - untouched"),
            new Row("true", "true", "booleans are never coerced (D-NC4)"),
            new Row("\"5000\"", "\"5000\"", "numeric-looking strings are never coerced (D-NC4)"),
            new Row("\"5000.0\"", "\"5000.0\"", "... including ones that look like the defect"),
            new Row("null", "null", "null is never coerced"),
        };
        for (Row row : rows) {
            assertEquals("{\"v\":" + row.expected() + "}",
                    canon("{\"v\":" + row.input() + "}"),
                    row.input() + " -> " + row.expected() + " (" + row.rationale() + ")");
        }
    }

    // ------------------------------------------------------------------
    // Structural guarantees
    // ------------------------------------------------------------------

    @Test
    void rewritesNestedObjectsAndArraysAtEveryDepth() {
        assertEquals("{\"a\":{\"b\":[1,{\"c\":2},[3,[4]]],\"d\":5}}",
                canon("{\"a\":{\"b\":[1.0,{\"c\":2.0},[3.0,[4.0]]],\"d\":5.0}}"));
    }

    @Test
    void leavesObjectKeysAlone() {
        // Keys are strings; a key that looks like a number must survive verbatim.
        assertEquals("{\"5000.0\":5000,\"1e3\":1}", canon("{\"5000.0\":5000.0,\"1e3\":1.0}"));
    }

    @Test
    void isIdempotent() {
        String source = """
                {"int":5000,"double":5000.0,"frac":5000.5,"big":1e19,"huge":1e20,\
                "str":"5000","bool":true,"nil":null,"arr":[1.0,2.5]}""";
        String once = canon(source);
        String twice = ConfigNumbers.canonicalizeNumbers(JsonParser.parseString(once)).toString();
        assertEquals(once, twice, "a canonical document must be a fixed point");
    }

    @Test
    void canonicalizedCopyDoesNotMutateItsSource() {
        JsonObject source = JsonParser.parseString("{\"v\":5000.0}").getAsJsonObject();
        JsonObject copy = ConfigNumbers.canonicalized(source);

        assertEquals("{\"v\":5000}", copy.toString());
        assertEquals("{\"v\":5000.0}", source.toString(),
                "the caller's document must never be rewritten behind its back");
    }

    @Test
    void nonFiniteNumbersAreLeftUntouched() {
        // NaN/infinity have no JSON literal; they only reach a Gson tree when a primitive is built
        // programmatically. They are not canonicalizable, so they survive to fail downstream.
        JsonPrimitive nan = new JsonPrimitive(Double.NaN);
        JsonPrimitive positiveInfinity = new JsonPrimitive(Double.POSITIVE_INFINITY);
        JsonPrimitive negativeInfinity = new JsonPrimitive(Double.NEGATIVE_INFINITY);

        assertSame(nan, ConfigNumbers.canonicalizeNumbers(nan));
        assertSame(positiveInfinity, ConfigNumbers.canonicalizeNumbers(positiveInfinity));
        assertSame(negativeInfinity, ConfigNumbers.canonicalizeNumbers(negativeInfinity));
    }

    @Test
    void nullsAndEmptyDocumentsAreHandled() {
        assertNull(ConfigNumbers.canonicalized(null));
        assertNull(ConfigNumbers.canonicalizeNumbers(null));
        assertSame(JsonNull.INSTANCE, ConfigNumbers.canonicalizeNumbers(JsonNull.INSTANCE));
        assertEquals("{}", ConfigNumbers.canonicalized(new JsonObject()).toString());
    }

    @Test
    void aBareNumberElementIsReturnedInCanonicalForm() {
        // Objects/arrays are rewritten in place; a bare primitive has no container to write into,
        // so the caller must use the returned element.
        assertEquals("5000",
                ConfigNumbers.canonicalizeNumbers(JsonParser.parseString("5000.0")).toString());
    }

    // ------------------------------------------------------------------
    // D-NC2 - strict reads: loud rejection, never silent rewriting
    // ------------------------------------------------------------------

    @Test
    void strictReadAcceptsAnyIntegralEncoding() {
        String[][] accepted = {
            {"5", "5"}, {"5.0", "5"}, {"5.00", "5"}, {"5e0", "5"},
            {"0", "0"}, {"-0.0", "0"}, {"2147483647", "2147483647"},
        };
        for (String[] pair : accepted) {
            assertEquals(Integer.parseInt(pair[1]),
                    ConfigNumbers.requireNonNegativeInt(
                            JsonParser.parseString(pair[0]), "heartbeat.intervalSecs"),
                    pair[0] + " must read as " + pair[1]);
        }
    }

    @Test
    void strictReadRejectsFractionalInsteadOfTruncating() {
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a whole number,"
                        + " but was 5.5",
                readError(JsonParser.parseString("5.5")));
    }

    @Test
    void strictReadRejectsNegativeInsteadOfClamping() {
        assertEquals("configuration value 'heartbeat.intervalSecs' must not be negative,"
                        + " but was -5",
                readError(JsonParser.parseString("-5")));
        // The float encoding of the same value is rejected identically.
        assertEquals("configuration value 'heartbeat.intervalSecs' must not be negative,"
                        + " but was -5.0",
                readError(JsonParser.parseString("-5.0")));
    }

    @Test
    void strictReadRejectsOutOfRangeForTheTargetType() {
        assertEquals("configuration value 'metricEmission.targetConfig.port' is out of range"
                        + " for a 32-bit integer: 2147483648",
                portError(JsonParser.parseString("2147483648")));
        // Beyond the canonicalization window too (the value never became an integer primitive).
        assertEquals("configuration value 'metricEmission.targetConfig.port' is out of range"
                        + " for a 32-bit integer: 100000000000000000000",
                portError(JsonParser.parseString("1e20")));
    }

    @Test
    void strictReadRejectsNonFiniteNumbers() {
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a finite number,"
                        + " but was NaN",
                readError(new JsonPrimitive(Double.NaN)));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a finite number,"
                        + " but was Infinity",
                readError(new JsonPrimitive(Double.POSITIVE_INFINITY)));
    }

    @Test
    void strictReadRejectsNonNumbersWithTheOffendingValueNamed() {
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was \"5000\"", readError(JsonParser.parseString("\"5000\"")));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was true", readError(JsonParser.parseString("true")));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was null", readError(JsonNull.INSTANCE));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was absent", readError(null));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was an object", readError(JsonParser.parseString("{\"a\":1}")));
        assertEquals("configuration value 'heartbeat.intervalSecs' must be a number,"
                + " but was an array", readError(JsonParser.parseString("[1]")));
    }

    private static String readError(JsonElement element) {
        return assertThrows(IllegalArgumentException.class,
                () -> ConfigNumbers.requireNonNegativeInt(element, "heartbeat.intervalSecs"))
                .getMessage();
    }

    private static String portError(JsonElement element) {
        return assertThrows(IllegalArgumentException.class,
                () -> ConfigNumbers.requireNonNegativeInt(element, "metricEmission.targetConfig.port"))
                .getMessage();
    }
}
