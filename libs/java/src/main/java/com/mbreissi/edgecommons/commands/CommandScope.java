/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.commands;

import java.util.Locale;

/**
 * The declared <b>scope</b> of a registered command verb (DESIGN-scoped-commands D-SC-2): which
 * {@code cmd} addressing a verb accepts. Every registration
 * ({@link CommandInbox#register(String, CommandScope, CommandHandler)} /
 * {@link CommandInbox#registerOutcome(String, CommandScope, OutcomeCommandHandler)}) declares one,
 * the library enforces it <b>before dispatch</b> (the handler never runs on an addressing error),
 * and {@code describe} advertises it as the lowercase {@link #wireName()}.
 *
 * <ul>
 *   <li>{@link #COMPONENT} — component-addressed only ({@code ecv1/{d}/{c}/cmd/{verb}}). An
 *       instance-addressed delivery, or a body naming an instance, is rejected with
 *       {@code BAD_ARGS}. The handler’s {@code addressedInstance} is always {@code null}.</li>
 *   <li>{@link #INSTANCE} — instance-targeted. The handler receives the topic’s instance token,
 *       else the body’s {@code instance} field, else {@code null} (the component applies its own
 *       default/unknown-instance policy on {@code null}).</li>
 *   <li>{@link #BOTH} — dual-scope: the same resolution as {@code INSTANCE}, but {@code null}
 *       carries meaning — “addressed to the whole component” (D-SC-3).</li>
 * </ul>
 */
public enum CommandScope {

    /** Component-addressed only; instance addressing (topic or body) is rejected. */
    COMPONENT,

    /** Instance-targeted; the addressed instance resolves topic-first, then body, else null. */
    INSTANCE,

    /** Valid at either scope; a null addressed instance means "the whole component". */
    BOTH;

    /** The lowercase wire form advertised in {@code describe} ({@code "component"} etc.). */
    public String wireName() {
        return name().toLowerCase(Locale.ROOT);
    }
}
