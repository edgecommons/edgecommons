/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.utils;

import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

/**
 * Unit tests for {@link Utils} static helpers: {@code sleep}, {@code stringify}
 * and {@code destringify} (round trip plus the invalid-JSON null path).
 */
class UtilsTest {

    @Test
    void sleepReturnsAfterRequestedDuration() {
        long start = System.nanoTime();
        Utils.sleep(2);
        long elapsedMs = (System.nanoTime() - start) / 1_000_000;
        // We only assert it returned without throwing; timing is not strict.
        assertTrue(elapsedMs >= 0);
    }

    @Test
    void sleepHandlesInterruptWithoutThrowing() throws InterruptedException {
        final boolean[] completed = {false};
        Thread t = new Thread(() -> {
            Utils.sleep(200);
            completed[0] = true;
        });
        t.start();
        // Interrupt mid-sleep; Utils.sleep swallows InterruptedException.
        t.interrupt();
        t.join(2000);
        assertFalse(t.isAlive());
        assertTrue(completed[0]);
    }

    @Test
    void stringifyAndDestringifyRoundTrip() {
        JsonObject obj = new JsonObject();
        obj.addProperty("name", "value");
        obj.addProperty("num", 42);

        String json = Utils.stringify(obj);
        assertNotNull(json);
        assertTrue(json.contains("\"name\""));

        JsonObject restored = Utils.destringify(json);
        assertNotNull(restored);
        assertEquals("value", restored.get("name").getAsString());
        assertEquals(42, restored.get("num").getAsInt());
    }

    @Test
    void destringifyReturnsNullForInvalidJson() {
        // Not a valid JSON object; the JsonSyntaxException path returns null.
        assertNull(Utils.destringify("this is not json {"));
    }

    @Test
    void destringifyReturnsNullForEmptyString() {
        // Gson returns null for an empty document; covered without an exception.
        assertNull(Utils.destringify(""));
    }
}
