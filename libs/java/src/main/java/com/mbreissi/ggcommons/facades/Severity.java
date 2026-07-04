/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.facades;

import java.util.Locale;

/**
 * The operator-event severity taxonomy (DESIGN-class-facades §2.2). The wire token is the enum's
 * <b>lowercase</b> name — {@code critical | warning | info | debug} — and it is <b>the first
 * channel token</b> of every {@code evt} publish: {@link com.mbreissi.ggcommons.facades.EventsFacade}
 * derives the channel {@code evt/{severity}/{type}} from the body's own severity + type, so the
 * topic and the body can never disagree. A console subscribes {@code ecv1/+/+/+/evt/critical/#} for
 * just alarms.
 *
 * <p><b>Mirror note (Python/Rust/TS):</b> {@code str}/{@code #[serde(rename_all="lowercase")]}/
 * {@code enum} with the identical four lowercase wire tokens.
 */
public enum Severity {
    /** An alarm-grade condition demanding operator attention (the {@code raiseAlarm} default). */
    CRITICAL,
    /** A degraded but non-critical condition. */
    WARNING,
    /** An informational event (the message-only {@code emit} default). */
    INFO,
    /** A diagnostic event. */
    DEBUG;

    /** The wire token — the lowercase enum name, the {@code evt} channel's first token. */
    public String wire() {
        return name().toLowerCase(Locale.ROOT);
    }

    /**
     * Resolves a lowercase wire token to its severity.
     *
     * @param token the lowercase wire token (e.g. {@code "critical"})
     * @return the matching severity, or {@code null} when the token is outside the closed set
     */
    public static Severity fromWire(String token) {
        for (Severity s : values()) {
            if (s.wire().equals(token)) {
                return s;
            }
        }
        return null;
    }
}
