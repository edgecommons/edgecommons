/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Covers the validation / defaulting branches of {@link GGCommonsBuilder#build()} that the
 * happy-path lifecycle test does not reach: the null-component-name guard and the null-args
 * default. The null-args case still attempts a real {@code init()} (which fails without a
 * Greengrass IPC environment), so it is asserted to throw {@link RuntimeException} from init
 * rather than the {@link IllegalStateException} the name guard throws first.
 */
class GGCommonsBuilderValidationTest {

    @Test
    void buildWithNullComponentNameThrowsIllegalState() {
        // create(null) does not validate; build() must.
        GGCommonsBuilder builder = GGCommonsBuilder.create(null).withArgs(new String[0]);
        IllegalStateException ex = assertThrows(IllegalStateException.class, builder::build);
        assertTrue(ex.getMessage().contains("Component name is required"));
    }

    @Test
    void buildWithNullArgsDefaultsToEmptyThenAttemptsInit() {
        // No withArgs() call -> args is null -> build() substitutes an empty array (covering the
        // defaulting branch) before calling init(), which then fails (no IPC) and rethrows.
        GGCommonsBuilder builder = GGCommonsBuilder.create("com.test.NullArgs");
        RuntimeException ex = assertThrows(RuntimeException.class, builder::build);
        // The failure comes from init(), proving the null-args default branch was taken first
        // (otherwise an NPE on args would have surfaced instead).
        assertTrue(ex.getMessage().contains("Failed to initialize GGCommons"));
    }
}
