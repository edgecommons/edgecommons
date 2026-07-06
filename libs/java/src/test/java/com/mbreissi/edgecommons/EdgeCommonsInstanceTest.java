/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.messaging.Message;
import com.mbreissi.edgecommons.messaging.MessageIdentity;
import com.mbreissi.edgecommons.uns.Uns;
import com.mbreissi.edgecommons.uns.UnsClass;
import com.mbreissi.edgecommons.uns.UnsValidationException;
import org.junit.jupiter.api.Test;

import java.util.Collection;
import java.util.List;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertNotSame;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;

/**
 * Unit tests for the instance seam (UNS-CANONICAL-DESIGN §3): {@link EdgeCommons#instance(String)}
 * (token validation, per-id caching, dynamic ids), the {@link EdgeCommonsInstance} handle ({@code uns()}
 * binding, {@code newMessage} identity stamping) and {@link EdgeCommons#getUns()} (component
 * binding, {@code topic.includeRoot} wiring, lazy caching, uninitialized guard).
 */
class EdgeCommonsInstanceTest {

    private static final MessageIdentity IDENTITY = new MessageIdentity(
            List.of(new MessageIdentity.HierEntry("site", "dallas"),
                    new MessageIdentity.HierEntry("device", "gw-01")), "opcua-adapter", "main");

    /** Config stub: identity + includeRoot + configured instance ids, no real config source. */
    private static final class StubConfigManager extends ConfigManager {
        private final MessageIdentity identity;
        private final boolean includeRoot;
        private final Collection<String> instanceIds;

        StubConfigManager(MessageIdentity identity, boolean includeRoot, Collection<String> instanceIds) {
            this.identity = identity;
            this.includeRoot = includeRoot;
            this.instanceIds = instanceIds;
        }

        @Override
        public MessageIdentity getComponentIdentity() {
            return identity;
        }

        @Override
        public boolean isTopicIncludeRoot() {
            return includeRoot;
        }

        @Override
        public Collection<String> getInstanceIds() {
            return instanceIds;
        }
    }

    private static EdgeCommons gg(ConfigManager configManager) {
        EdgeCommons gg = new EdgeCommons();
        gg.configManager = configManager;
        return gg;
    }

    private static EdgeCommons gg() {
        return gg(new StubConfigManager(IDENTITY, false, List.of("kep1")));
    }

    // ----- EdgeCommons.instance(): token validation -----

    @Test
    void instanceValidatesTheTokenAgainstTheTokenRule() {
        EdgeCommons gg = gg();
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                assertThrows(UnsValidationException.class, () -> gg.instance(null)).getCode());
        assertEquals(UnsValidationException.Code.EMPTY_TOKEN,
                assertThrows(UnsValidationException.class, () -> gg.instance("")).getCode());
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                assertThrows(UnsValidationException.class, () -> gg.instance("a/b")).getCode());
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                assertThrows(UnsValidationException.class, () -> gg.instance("a+b")).getCode());
        assertEquals(UnsValidationException.Code.BAD_CHAR,
                assertThrows(UnsValidationException.class, () -> gg.instance("a#b")).getCode());
        assertEquals(UnsValidationException.Code.TRAVERSAL,
                assertThrows(UnsValidationException.class, () -> gg.instance("a..b")).getCode());
    }

    @Test
    void sanitizerLegalTokensAreAccepted() {
        EdgeCommons gg = gg();
        // Spaces and dots survive the template sanitizer, so they are legal instance tokens.
        assertEquals("kep 1", gg.instance("kep 1").id());
        assertEquals("v1.2", gg.instance("v1.2").id());
    }

    // ----- EdgeCommons.instance(): caching + dynamic ids -----

    @Test
    void handlesAreCachedPerId() {
        EdgeCommons gg = gg();
        EdgeCommonsInstance first = gg.instance("kep1");
        assertSame(first, gg.instance("kep1"));
        assertNotSame(first, gg.instance("kep2"));
    }

    @Test
    void unknownIdsStillCreateAHandle() {
        // The id is deliberately NOT verified against component.instances[] — instances may be
        // created dynamically; an unknown id is only a DEBUG diagnostic.
        EdgeCommons gg = gg(); // configured instances: ["kep1"]
        EdgeCommonsInstance dynamic = gg.instance("not-configured");
        assertNotNull(dynamic);
        assertEquals("not-configured", dynamic.id());
    }

    @Test
    void nullConfiguredInstanceIdsAreTolerated() {
        EdgeCommons gg = gg(new StubConfigManager(IDENTITY, false, null));
        assertEquals("kep1", gg.instance("kep1").id());
    }

    // ----- EdgeCommonsInstance: uns() binding -----

    @Test
    void unsIsBoundToTheInstanceToken() {
        EdgeCommons gg = gg();
        EdgeCommonsInstance kep1 = gg.instance("kep1");
        assertEquals("kep1", kep1.uns().identity().getInstance());
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/state", kep1.uns().topic(UnsClass.STATE));
        assertEquals("ecv1/gw-01/opcua-adapter/kep1/data/temp",
                kep1.uns().topic(UnsClass.DATA, "temp"));
        // The component identity is otherwise untouched (same hier/component).
        assertEquals(IDENTITY.getPath(), kep1.uns().identity().getPath());
        assertEquals(IDENTITY.getComponent(), kep1.uns().identity().getComponent());
    }

    @Test
    void instanceHandleRespectsIncludeRoot() {
        EdgeCommons gg = gg(new StubConfigManager(IDENTITY, true, List.of()));
        assertEquals("ecv1/dallas/gw-01/opcua-adapter/kep1/state",
                gg.instance("kep1").uns().topic(UnsClass.STATE));
    }

    // ----- EdgeCommonsInstance: newMessage() stamping -----

    @Test
    void newMessageStampsTheInstanceIdentity() {
        EdgeCommons gg = gg();
        Message msg = gg.instance("kep1").newMessage("reading", "1.0").build();
        assertNotNull(msg.getIdentity());
        assertEquals("kep1", msg.getIdentity().getInstance());
        assertEquals(IDENTITY.getPath(), msg.getIdentity().getPath());
        assertEquals(IDENTITY.getComponent(), msg.getIdentity().getComponent());
        assertEquals("reading", msg.getHeader().getName());
        assertEquals("1.0", msg.getHeader().getVersion());
    }

    @Test
    void componentLevelMessagesStayOnMain() {
        // Contrast: a message built without the handle defaults to instance "main".
        EdgeCommons gg = gg();
        Message msg = com.mbreissi.edgecommons.messaging.MessageBuilder.create("reading", "1.0")
                .withConfig(gg.getConfigManager())
                .build();
        assertEquals(MessageIdentity.DEFAULT_INSTANCE, msg.getIdentity().getInstance());
    }

    // ----- EdgeCommons.getUns() -----

    @Test
    void getUnsIsBoundToTheComponentIdentityOnMain() {
        EdgeCommons gg = gg();
        Uns uns = gg.getUns();
        assertSame(IDENTITY, uns.identity());
        assertEquals("main", uns.identity().getInstance());
        assertEquals("ecv1/gw-01/opcua-adapter/main/state", uns.topic(UnsClass.STATE));
    }

    @Test
    void getUnsIsCached() {
        EdgeCommons gg = gg();
        assertSame(gg.getUns(), gg.getUns());
    }

    @Test
    void getUnsRespectsIncludeRoot() {
        EdgeCommons gg = gg(new StubConfigManager(IDENTITY, true, List.of()));
        assertEquals("ecv1/dallas/gw-01/opcua-adapter/main/state",
                gg.getUns().topic(UnsClass.STATE));
        assertEquals("ecv1/+/+/+/+/data/#",
                gg.getUns().filter(UnsClass.DATA, com.mbreissi.edgecommons.uns.UnsScope.all()));
    }

    // ----- uninitialized guards -----

    @Test
    void unsAccessorsRequireInitialization() {
        EdgeCommons uninitialized = new EdgeCommons();
        assertThrows(IllegalStateException.class, uninitialized::getUns);
        assertThrows(IllegalStateException.class, () -> uninitialized.instance("kep1"));

        // A config manager without a resolved identity (test/subclass bring-up) is equally not
        // ready for UNS topic building.
        EdgeCommons noIdentity = gg(new ConfigManager() { });
        assertNull(noIdentity.getConfigManager().getComponentIdentity());
        assertThrows(IllegalStateException.class, noIdentity::getUns);
        assertThrows(IllegalStateException.class, () -> noIdentity.instance("kep1"));
    }
}
