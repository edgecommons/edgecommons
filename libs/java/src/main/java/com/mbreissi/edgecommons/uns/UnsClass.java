/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.uns;

import java.util.Set;

/**
 * The closed UNS class set (UNS-CANONICAL-DESIGN §2.1) — the fifth topic level of every UNS
 * topic ({@code ecv1[/{site}]/{device}/{component}/{instance}/{class}[/{channel…}]}).
 *
 * <p>Each class is either a <b>leaf</b> (the class token is the last topic level — a channel is
 * forbidden) or <b>channeled</b> (at least one channel token is REQUIRED after the class).
 * {@link #RESERVED} lists the library-owned publish classes ({@code state}, {@code metric},
 * {@code cfg}, {@code log}) that components must not publish to directly (enforcement lands with
 * the reserved-class publish guard).
 */
public enum UnsClass {
    /** Component liveness/state keepalive (library-owned). Leaf. */
    STATE("state", true),
    /** Component metrics (library-owned). Channeled. */
    METRIC("metric", false),
    /** Effective-configuration announcements (library-owned). Leaf. */
    CFG("cfg", true),
    /** Log tailing (library-owned; publisher lands in a later phase). Channeled. */
    LOG("log", false),
    /** Application telemetry/data. Channeled. */
    DATA("data", false),
    /** Application events. Channeled. */
    EVT("evt", false),
    /** Command inboxes (request/reply verbs). Channeled. */
    CMD("cmd", false),
    /** Free-form application namespace. Channeled. */
    APP("app", false);

    /** The wire token — the class topic level exactly as it appears in a topic. */
    public final String token;

    /** Leaf semantics: {@code true} — channel forbidden; {@code false} — channel REQUIRED. */
    public final boolean leaf;

    /** The library-owned publish classes ({@code state | metric | cfg | log}). */
    public static final Set<UnsClass> RESERVED = Set.of(STATE, METRIC, CFG, LOG);

    UnsClass(String token, boolean leaf) {
        this.token = token;
        this.leaf = leaf;
    }

    /**
     * Resolves a wire token to its class.
     *
     * @param token the class topic-level token (e.g. {@code "state"})
     * @return the matching class, or {@code null} when the token is outside the closed set
     */
    public static UnsClass fromToken(String token) {
        for (UnsClass cls : values()) {
            if (cls.token.equals(token)) {
                return cls;
            }
        }
        return null;
    }
}
