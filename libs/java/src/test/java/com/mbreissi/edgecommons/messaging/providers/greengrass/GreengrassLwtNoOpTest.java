/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.greengrass;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;

/**
 * MQTT LWT on the Greengrass IPC transport is an explicit <b>no-op</b>
 * (UNS-CANONICAL-DESIGN §6): IPC has no CONNECT packet to register a will on, so any configured
 * {@code messaging.lwt} section is ignored with a DEBUG notice. The provider itself requires a
 * live Nucleus to construct (and is excluded from the coverage gate), so this exercises the
 * static no-op notice directly.
 */
class GreengrassLwtNoOpTest {

    @Test
    void lwtNoOpNoticeIsSafe() {
        assertDoesNotThrow(GreengrassMessagingProvider::logLwtNoOp);
    }
}
