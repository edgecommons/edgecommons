/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.config;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;

import java.math.BigDecimal;
import java.math.BigInteger;
import java.util.Map;

/**
 * The canonical numeric contract for EdgeCommons configuration documents (D-NC1 / D-NC2).
 *
 * <p>Configuration reaches a component through several stores, and they disagree about how a
 * JSON number is encoded. A file or ConfigMap delivers {@code 5000}; a store that round-trips
 * numbers through a 64-bit float delivers the same setting as {@code 5000.0}. Without a shared
 * rule the <em>same</em> configuration is a different document depending on where it came from:
 * a strict parser rejects one encoding, a lenient one silently truncates, and the effective
 * configuration published on the UNS {@code cfg} class differs per language and per platform.
 *
 * <p>This class fixes the rule in one place, at the configuration intake boundary
 * ({@link ConfigManager}), so that every downstream consumer — the library's own config classes,
 * component-supplied candidate validators, {@code getFullConfig()} / {@code getGlobalConfig()} /
 * {@code getInstanceConfig(..)}, the envelope {@code tags}, and the published {@code cfg}
 * document — sees one canonical form.
 *
 * <h2>Canonicalization (D-NC1)</h2>
 *
 * {@link #canonicalizeNumbers(JsonElement)} walks a document and rewrites every JSON number whose
 * exact decimal value is <b>integral</b> and lies inside the shared 64-bit window
 * {@code [-2^63, 2^64 - 1]} into an integer JSON number. Everything else is left byte-identical:
 * fractional numbers, numbers outside the window, non-finite numbers, strings, booleans, and
 * {@code null}. Object keys are never touched. Strings that merely <em>look</em> numeric
 * ({@code "5000"}) are never coerced (D-NC4) — coercing them would corrupt legitimately-string
 * settings such as version literals.
 *
 * <p>Exactness is decided on the <b>decimal</b> value, not on a double: Gson keeps the source text
 * of a parsed number, so {@code 5000.00} and {@code 5e3} are both recognized as the integer
 * {@code 5000} without a lossy intermediate conversion.
 *
 * <p>The pass is pure (it derives the result from its input alone) and idempotent — a canonical
 * document is a fixed point, so applying it twice is harmless.
 *
 * <h2>Strict reads (D-NC2)</h2>
 *
 * Canonicalization widens what parses; it never rewrites a value the author did not write.
 * A fractional value in an integer-typed setting, a negative value in a non-negative setting, and
 * a value outside the target type's range are <b>rejected loudly</b> by
 * {@link #requireNonNegativeInt(JsonElement, String)} rather than silently truncated or clamped.
 * Silently rewriting a configured value is a worse defect than refusing to start.
 *
 * @see ConfigManager
 */
public final class ConfigNumbers {

    /** Lower bound of the canonicalization window: {@code -2^63} ({@code i64::MIN}). */
    private static final BigDecimal MIN_SIGNED_64 = new BigDecimal(Long.MIN_VALUE);
    /** Largest value representable as a signed 64-bit integer: {@code 2^63 - 1}. */
    private static final BigDecimal MAX_SIGNED_64 = new BigDecimal(Long.MAX_VALUE);
    /** Upper bound of the canonicalization window: {@code 2^64 - 1} ({@code u64::MAX}). */
    private static final BigDecimal MAX_UNSIGNED_64 =
            new BigDecimal(BigInteger.ONE.shiftLeft(64).subtract(BigInteger.ONE));

    /** Uniform prefix for every numeric-configuration rejection. */
    private static final String ERROR_PREFIX = "configuration value ";

    private ConfigNumbers() {
    }

    /**
     * Returns a canonicalized deep copy of {@code config}, leaving the argument untouched.
     *
     * <p>The copy-then-rewrite shape is what the configuration intake boundary needs: a candidate
     * document supplied by a provider (or by a caller of {@link ConfigManager#applyConfig}) must
     * never be mutated behind the caller's back, and the committed snapshot must be isolated from
     * later edits to the source object.
     *
     * @param config the document to canonicalize (may be {@code null})
     * @return a canonical deep copy, or {@code null} when {@code config} is {@code null}
     */
    public static JsonObject canonicalized(JsonObject config) {
        if (config == null) {
            return null;
        }
        JsonObject copy = config.deepCopy();
        canonicalizeNumbers(copy);
        return copy;
    }

    /**
     * Rewrites every integral JSON number in {@code element} into integer form, in place.
     *
     * <p>Objects and arrays are rewritten in place and returned as-is; a bare number element has no
     * container to write into, so the canonical replacement is returned instead. Callers holding a
     * container therefore need only call this method and may ignore the return value; callers
     * holding a bare primitive must use the returned element.
     *
     * <p>A number is rewritten only when its exact decimal value has no fractional part
     * <em>and</em> lies inside {@code [-2^63, 2^64 - 1]}. Values in {@code [-2^63, 2^63 - 1]}
     * become a {@code long}-backed primitive; values in {@code (2^63 - 1, 2^64 - 1]} become a
     * {@link BigInteger}-backed primitive, so the whole unsigned 64-bit range survives. Anything
     * else — a fractional value, a value outside the window, {@code NaN}, an infinity, a string,
     * a boolean, {@code null} — is returned unchanged.
     *
     * <p>Idempotent: canonical input is a fixed point.
     *
     * @param element the document (or element) to canonicalize; {@code null} is returned as-is
     * @return the canonical element ({@code element} itself for objects and arrays)
     */
    public static JsonElement canonicalizeNumbers(JsonElement element) {
        if (element == null || element.isJsonNull()) {
            return element;
        }
        if (element.isJsonObject()) {
            for (Map.Entry<String, JsonElement> entry : element.getAsJsonObject().entrySet()) {
                entry.setValue(canonicalizeNumbers(entry.getValue()));
            }
            return element;
        }
        if (element.isJsonArray()) {
            JsonArray array = element.getAsJsonArray();
            for (int i = 0; i < array.size(); i++) {
                array.set(i, canonicalizeNumbers(array.get(i)));
            }
            return element;
        }
        JsonPrimitive primitive = element.getAsJsonPrimitive();
        if (!primitive.isNumber()) {
            return element;   // strings and booleans are never coerced (D-NC4)
        }
        BigDecimal integral = integralValue(primitive);
        if (integral == null) {
            return element;   // fractional, non-finite, or unparseable — left byte-identical
        }
        if (integral.compareTo(MIN_SIGNED_64) >= 0 && integral.compareTo(MAX_SIGNED_64) <= 0) {
            return new JsonPrimitive(integral.longValueExact());
        }
        if (integral.compareTo(MAX_SIGNED_64) > 0 && integral.compareTo(MAX_UNSIGNED_64) <= 0) {
            return new JsonPrimitive(integral.toBigIntegerExact());
        }
        return element;       // outside the shared 64-bit window — left byte-identical
    }

    /**
     * Reads a configuration number as a non-negative {@code int}, rejecting anything the author
     * did not actually write (D-NC2).
     *
     * <p>Accepts an integral value in any numeric encoding — {@code 5}, {@code 5.0}, {@code 5.00},
     * {@code 5e0} are the same value and all parse. Rejects, with an {@link IllegalArgumentException}
     * naming {@code path}:
     *
     * <ul>
     *   <li>a non-number (missing, {@code null}, string, boolean, object, array);</li>
     *   <li>{@code NaN} or an infinity;</li>
     *   <li>a fractional value — it is <b>never</b> truncated;</li>
     *   <li>a negative value — it is <b>never</b> clamped to zero or to a default;</li>
     *   <li>a value larger than {@link Integer#MAX_VALUE}.</li>
     * </ul>
     *
     * @param element the configured element (may be {@code null})
     * @param path    the dotted configuration path used in the error message, e.g.
     *                {@code "heartbeat.intervalSecs"}
     * @return the value as a non-negative {@code int}
     * @throws IllegalArgumentException if the value is not a non-negative whole number in range
     */
    public static int requireNonNegativeInt(JsonElement element, String path) {
        BigInteger value = requireNonNegativeIntegral(element, path);
        if (value.compareTo(BigInteger.valueOf(Integer.MAX_VALUE)) > 0) {
            throw new IllegalArgumentException(ERROR_PREFIX + quote(path)
                    + " is out of range for a 32-bit integer: " + value);
        }
        return value.intValueExact();
    }

    /**
     * The shared non-negative integral read behind {@link #requireNonNegativeInt}: validates the
     * element is a finite, whole, non-negative number and returns its exact value.
     */
    private static BigInteger requireNonNegativeIntegral(JsonElement element, String path) {
        if (element == null || element.isJsonNull() || !element.isJsonPrimitive()
                || !element.getAsJsonPrimitive().isNumber()) {
            throw new IllegalArgumentException(ERROR_PREFIX + quote(path)
                    + " must be a number, but was " + describe(element));
        }
        JsonPrimitive primitive = element.getAsJsonPrimitive();
        BigDecimal exact = exactValue(primitive);
        if (exact == null) {
            throw new IllegalArgumentException(ERROR_PREFIX + quote(path)
                    + " must be a finite number, but was " + primitive.getAsString());
        }
        BigDecimal integral = exact.stripTrailingZeros();
        if (integral.scale() > 0) {
            throw new IllegalArgumentException(ERROR_PREFIX + quote(path)
                    + " must be a whole number, but was " + primitive.getAsString());
        }
        BigInteger value = integral.toBigIntegerExact();
        if (value.signum() < 0) {
            throw new IllegalArgumentException(ERROR_PREFIX + quote(path)
                    + " must not be negative, but was " + primitive.getAsString());
        }
        return value;
    }

    /**
     * The exact decimal value of {@code primitive} when it has no fractional part and is finite,
     * otherwise {@code null}. Trailing zeros are stripped, so {@code 5000.0} and {@code 5000.00}
     * are both integral.
     */
    private static BigDecimal integralValue(JsonPrimitive primitive) {
        BigDecimal exact = exactValue(primitive);
        if (exact == null) {
            return null;
        }
        BigDecimal stripped = exact.stripTrailingZeros();
        return stripped.scale() > 0 ? null : stripped;
    }

    /**
     * The exact decimal value of a numeric primitive, or {@code null} when it has none —
     * {@code NaN} and the infinities (which only reach a Gson tree when a primitive is built
     * programmatically from a {@code double}/{@code float}), and any literal Gson refuses to parse
     * as a {@link BigDecimal}.
     */
    private static BigDecimal exactValue(JsonPrimitive primitive) {
        try {
            return primitive.getAsBigDecimal();
        } catch (NumberFormatException e) {
            return null;
        }
    }

    /** Renders a rejected element for an error message without ever throwing. */
    private static String describe(JsonElement element) {
        if (element == null) {
            return "absent";
        }
        if (element.isJsonNull()) {
            return "null";
        }
        if (element.isJsonObject()) {
            return "an object";
        }
        if (element.isJsonArray()) {
            return "an array";
        }
        return element.toString();   // quotes strings, renders booleans as literals
    }

    private static String quote(String path) {
        return "'" + path + "'";
    }
}
