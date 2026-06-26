/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.metrics.MetricEmitter;
import com.mbreissi.ggcommons.streaming.StreamMetricsBridge;
import com.mbreissi.ggcommons.streaming.StreamService;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.Test;

import java.lang.reflect.Field;
import java.lang.reflect.Method;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

/**
 * Unit tests for the root {@link GGCommons} facade that do NOT require a broker or native
 * library. They drive the parts the broker-backed {@code GGCommonsLifecycleTest} cannot reach:
 * <ul>
 *   <li>the deprecated public constructors, which call {@code init(..)} and must wrap the failure
 *       to bring up a real Greengrass IPC environment in a {@link RuntimeException} (init's
 *       catch-and-rethrow);</li>
 *   <li>the {@code initStreaming()} early-return guard for both an absent {@code streaming} section
 *       and a present-but-non-object one (driven reflectively against a mocked config manager);</li>
 *   <li>the {@code shutdown()} branches that close the streaming service and stream-metrics bridge
 *       (only reachable when those fields are populated), via injected mocks;</li>
 *   <li>the {@code processArgs} {@link org.apache.commons.cli.ParseException} branch (an unknown
 *       short option), which is swallowed and logged rather than thrown.</li>
 * </ul>
 * Field/method injection uses reflection against the protected no-arg constructor so nothing
 * connects to IPC/MQTT.
 */
class GGCommonsFacadeUnitTest {

    private static void setField(GGCommons gg, String name, Object value) throws Exception {
        Field f = GGCommons.class.getDeclaredField(name);
        f.setAccessible(true);
        f.set(gg, value);
    }

    private static GGCommons newBare() throws Exception {
        var ctor = GGCommons.class.getDeclaredConstructor();
        ctor.setAccessible(true);
        return ctor.newInstance();
    }

    private static void invokePrivate(GGCommons gg, String method) throws Exception {
        Method m = GGCommons.class.getDeclaredMethod(method);
        m.setAccessible(true);
        m.invoke(gg);
    }

    // ----- deprecated constructors (init -> RuntimeException wrap) -----

    @SuppressWarnings("deprecation")
    @Test
    void deprecatedTwoArgConstructorWrapsInitFailure() {
        // No Greengrass IPC environment available -> init() must fail and rethrow as RuntimeException.
        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> new GGCommons("com.test.Dep2", new String[]{"--platform", "GREENGRASS"}));
        assertTrue(ex.getMessage().contains("Failed to initialize GGCommons"));
    }

    @SuppressWarnings("deprecation")
    @Test
    void deprecatedThreeArgConstructorWrapsInitFailure() {
        org.apache.commons.cli.Options opts = new org.apache.commons.cli.Options();
        assertThrows(RuntimeException.class,
                () -> new GGCommons("com.test.Dep3", new String[]{"--platform", "GREENGRASS"}, opts));
    }

    @SuppressWarnings("deprecation")
    @Test
    void deprecatedFourArgConstructorWrapsInitFailure() {
        org.apache.commons.cli.Options opts = new org.apache.commons.cli.Options();
        assertThrows(RuntimeException.class,
                () -> new GGCommons("com.test.Dep4", new String[]{"--platform", "GREENGRASS"}, opts, false));
    }

    // ----- initStreaming early-return guard -----

    @Test
    void initStreamingNoOpWhenSectionAbsent() throws Exception {
        GGCommons gg = newBare();
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getFullConfig()).thenReturn(new JsonObject()); // no "streaming" key
        setField(gg, "configManager", cm);

        invokePrivate(gg, "initStreaming");

        assertNull(gg.getStreams(), "no streaming section -> streams stays null");
    }

    @Test
    void initStreamingNoOpWhenFullConfigNull() throws Exception {
        GGCommons gg = newBare();
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getFullConfig()).thenReturn(null);
        setField(gg, "configManager", cm);

        invokePrivate(gg, "initStreaming");

        assertNull(gg.getStreams());
    }

    @Test
    void initStreamingNoOpWhenStreamingIsNotObject() throws Exception {
        GGCommons gg = newBare();
        JsonObject full = new JsonObject();
        full.addProperty("streaming", "not-an-object"); // present but wrong type
        ConfigManager cm = mock(ConfigManager.class);
        when(cm.getFullConfig()).thenReturn(full);
        setField(gg, "configManager", cm);

        invokePrivate(gg, "initStreaming");

        assertNull(gg.getStreams());
    }

    // ----- shutdown branches that close streaming -----

    @Test
    void shutdownClosesStreamingServiceAndBridge() throws Exception {
        GGCommons gg = newBare();

        StreamMetricsBridge bridge = mock(StreamMetricsBridge.class);
        StreamService streams = mock(StreamService.class);
        MetricEmitter metrics = mock(MetricEmitter.class);
        MessagingClient messaging = mock(MessagingClient.class);
        ConfigManager cm = mock(ConfigManager.class);

        setField(gg, "streamMetricsBridge", bridge);
        setField(gg, "streams", streams);
        setField(gg, "metricEmitter", metrics);
        setField(gg, "messagingClient", messaging);
        setField(gg, "configManager", cm);

        gg.shutdown();

        verify(bridge).close();
        verify(streams).close();
        verify(metrics).close();
        verify(messaging).close();
        verify(cm).close();
    }

    @Test
    void shutdownIsSafeWhenAllSubsystemsNull() throws Exception {
        GGCommons gg = newBare();
        // every field null -> every guarded branch is skipped without NPE
        assertDoesNotThrow(gg::shutdown);
    }

    // ----- processArgs ParseException branch -----

    @Test
    void processArgsSwallowsParseExceptionForUnknownOption() {
        // An unrecognized option triggers commons-cli ParseException, which processArgs
        // catches and logs; it returns a ParsedCommandLine with the resolved axes unset (null).
        ParsedCommandLine pcl = GGCommons.processArgs(
                "com.test.Comp", new String[]{"-zzz", "boom"}, null);

        assertNotNull(pcl);
        // The parse failed before the resolver ran, so platform/transport/config stay null.
        assertNull(pcl.platform);
        assertNull(pcl.transport);
        assertNull(pcl.configArgs);
        assertNull(pcl.commandLine);
    }
}
