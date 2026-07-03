/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.uns;

/**
 * The wildcard scope for {@link Uns#filter(UnsClass, UnsScope)} (UNS-CANONICAL-DESIGN §2.1).
 *
 * <p>A {@code null} field renders as the MQTT single-level wildcard {@code +} at that topic
 * position; a non-null field pins the position to that concrete token. The {@code site} field is
 * used only when the bound {@code topic.includeRoot} is {@code true} (the rooted grammar has a
 * site position between the {@value Uns#ROOT} root and the device); it is ignored otherwise.
 *
 * @param site      the first-hierarchy-level value to pin (rooted grammar only), or {@code null} for {@code +}
 * @param device    the device (thing) token to pin, or {@code null} for {@code +}
 * @param component the component token to pin, or {@code null} for {@code +}
 * @param instance  the instance token to pin, or {@code null} for {@code +}
 */
public record UnsScope(String site, String device, String component, String instance) {

    /** Every position wildcarded — all devices, components and instances. */
    public static UnsScope all() {
        return new UnsScope(null, null, null, null);
    }

    /** All components/instances on one device. */
    public static UnsScope device(String device) {
        return new UnsScope(null, device, null, null);
    }

    /** All instances of one component on one device. */
    public static UnsScope component(String device, String component) {
        return new UnsScope(null, device, component, null);
    }

    /** One exact instance of one component on one device. */
    public static UnsScope instance(String device, String component, String instance) {
        return new UnsScope(null, device, component, instance);
    }
}
