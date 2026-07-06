/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.facades;

/**
 * The normalized, protocol-independent sample-quality verdict of the southbound contract
 * (DESIGN-class-facades §2.1, {@code docs/SOUTHBOUND.md} §3). The wire token is the enum's
 * <b>UPPERCASE</b> name — {@code GOOD | BAD | UNCERTAIN} — carried verbatim on every {@code data}
 * sample.
 *
 * <p>{@link com.mbreissi.edgecommons.facades.DataFacade} defaults an omitted sample quality to
 * {@link #GOOD} (marking the synthesis with {@code qualityRaw:"unspecified"}), so a sample can
 * never reach the bus without a quality — the structural guarantee the facade exists to make.
 *
 * <p><b>Mirror note (Python/Rust/TS):</b> {@code str}/{@code #[serde(rename_all="UPPERCASE")]}/
 * {@code enum} with the identical three UPPERCASE wire tokens.
 */
public enum Quality {
    /** The value is trustworthy (the default for a sample carrying a value with no verdict). */
    GOOD,
    /** The value is not trustworthy (exception/timeout/failed read). */
    BAD,
    /** The value is present but suspect (stale/partial). */
    UNCERTAIN;

    /** The wire token — the UPPERCASE enum name exactly as it appears in a {@code data} sample. */
    public String wire() {
        return name();
    }

    /**
     * Resolves a wire token to its quality.
     *
     * @param token the UPPERCASE wire token (e.g. {@code "GOOD"})
     * @return the matching quality, or {@code null} when the token is outside the closed set
     */
    public static Quality fromWire(String token) {
        for (Quality q : values()) {
            if (q.name().equals(token)) {
                return q;
            }
        }
        return null;
    }
}
