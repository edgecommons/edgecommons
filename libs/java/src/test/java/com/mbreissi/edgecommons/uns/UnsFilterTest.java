/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.uns;

import com.mbreissi.edgecommons.messaging.MessageIdentity;
import org.junit.jupiter.api.Test;

import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Unit tests for {@link Uns#filter(UnsClass, UnsScope)}: wildcard construction from
 * {@link UnsScope} (null field -> {@code +}), the {@code /#} channel tail for channeled classes,
 * the leaf-class flat filter, the rooted site position, and token validation of pinned fields.
 */
class UnsFilterTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    private static final Uns ROOTLESS = new Uns(IDENTITY, false);
    private static final Uns ROOTED = new Uns(IDENTITY, true);

    private static UnsValidationException.Code codeOf(org.junit.jupiter.api.function.Executable e) {
        return assertThrows(UnsValidationException.class, e).getCode();
    }

    // ----- scope factories -----

    @Test
    void scopeFactoriesFillTheExpectedPositions() {
        assertEquals(new UnsScope(null, null, null, null), UnsScope.all());
        assertEquals(new UnsScope(null, "gw-01", null, null), UnsScope.device("gw-01"));
        assertEquals(new UnsScope(null, "gw-01", "opcua-adapter", null),
                UnsScope.component("gw-01", "opcua-adapter"));
        assertEquals(new UnsScope(null, "gw-01", "opcua-adapter", "kep1"),
                UnsScope.instance("gw-01", "opcua-adapter", "kep1"));
    }

    // ----- rootless construction -----

    @Test
    void allScopeChanneledClassAppendsChannelWildcard() {
        assertEquals("ecv1/+/+/+/data/#", ROOTLESS.filter(UnsClass.DATA, UnsScope.all()));
        assertEquals("ecv1/+/+/+/metric/#", ROOTLESS.filter(UnsClass.METRIC, UnsScope.all()));
    }

    @Test
    void allScopeLeafClassEndsAtTheClassToken() {
        assertEquals("ecv1/+/+/+/state", ROOTLESS.filter(UnsClass.STATE, UnsScope.all()));
        assertEquals("ecv1/+/+/+/cfg", ROOTLESS.filter(UnsClass.CFG, UnsScope.all()));
    }

    @Test
    void deviceScopePinsTheDevicePosition() {
        assertEquals("ecv1/gw-01/+/+/data/#",
                ROOTLESS.filter(UnsClass.DATA, UnsScope.device("gw-01")));
    }

    @Test
    void componentScopePinsDeviceAndComponent() {
        assertEquals("ecv1/gw-01/opcua-adapter/+/evt/#",
                ROOTLESS.filter(UnsClass.EVT, UnsScope.component("gw-01", "opcua-adapter")));
    }

    @Test
    void instanceScopePinsAllThreePositions() {
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/cmd/#",
                ROOTLESS.filter(UnsClass.CMD, UnsScope.instance("gw-01", "opcua-adapter", "kep1")));
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/state",
                ROOTLESS.filter(UnsClass.STATE, UnsScope.instance("gw-01", "opcua-adapter", "kep1")));
    }

    // ----- includeRoot: the site position -----

    @Test
    void rootedFilterAddsAWildcardSitePosition() {
        assertEquals("ecv1/+/+/+/+/data/#", ROOTED.filter(UnsClass.DATA, UnsScope.all()));
        assertEquals("ecv1/+/+/+/+/state", ROOTED.filter(UnsClass.STATE, UnsScope.all()));
    }

    @Test
    void rootedFilterPinsAnExplicitSite() {
        assertEquals("ecv1/dallas/gw-01/+/+/data/#",
                ROOTED.filter(UnsClass.DATA, new UnsScope("dallas", "gw-01", null, null)));
    }

    @Test
    void rootlessFilterIgnoresTheSiteField() {
        // The site position exists only in the rooted grammar; a rootless filter has no
        // position to pin, so the field is ignored (§2.1: "site used only when includeRoot").
        assertEquals("ecv1/+/+/+/data/#",
                ROOTLESS.filter(UnsClass.DATA, new UnsScope("dallas", null, null, null)));
    }

    @Test
    void singleLevelHierarchyFilterHasNoSitePositionEvenWhenRooted() {
        // D-U25: includeRoot=true with a single-level bound hierarchy is a no-op — the filter
        // must match the (rootless) topics such a component actually builds.
        MessageIdentity single = new MessageIdentity(
                List.of(new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");
        Uns uns = new Uns(single, true);
        assertEquals("ecv1/+/+/+/data/#", uns.filter(UnsClass.DATA, UnsScope.all()));
        assertEquals("ecv1/+/+/+/data/#",
                uns.filter(UnsClass.DATA, new UnsScope("dallas", null, null, null)));
    }

    // ----- token validation of pinned fields -----

    @Test
    void pinnedScopeTokensMustPassTheTokenRule() {
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTLESS.filter(UnsClass.DATA, UnsScope.device("gw+1"))));
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                codeOf(() -> ROOTLESS.filter(UnsClass.DATA, UnsScope.component("gw-01", "a..b"))));
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                codeOf(() -> ROOTLESS.filter(UnsClass.DATA, UnsScope.device(""))));
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                codeOf(() -> ROOTED.filter(UnsClass.DATA, new UnsScope("dal#las", null, null, null))));
    }

    @Test
    void nullArgumentsAreRejected() {
        assertThrows(NullPointerException.class, () -> ROOTLESS.filter(null, UnsScope.all()));
        assertThrows(NullPointerException.class, () -> ROOTLESS.filter(UnsClass.DATA, null));
    }
}
