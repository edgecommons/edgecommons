/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.streaming;

import org.apache.logging.log4j.Level;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.invoke.MethodHandle;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_INT;

/**
 * Forwards the native {@code ggstreamlog} core's log events into log4j2, so streaming logs land in
 * the component's normal log stream instead of being dropped. Installed once when the native library
 * is loaded (see {@link GgStreamNative}); registers a Panama upcall as the C {@code ggsl_log_cb}.
 */
final class NativeLogBridge {

    private static final Logger SELF = LogManager.getLogger(NativeLogBridge.class);
    private static boolean installed;
    @SuppressWarnings("unused") // kept reachable so the upcall stub is not collected
    private static MemorySegment stub;

    private NativeLogBridge() {
    }

    /** Register the log-forwarding callback with the core. Idempotent. */
    static synchronized void install(GgStreamNative n) {
        if (installed) {
            return;
        }
        try {
            MethodHandle target = MethodHandles.lookup().findStatic(
                    NativeLogBridge.class, "onLog",
                    MethodType.methodType(void.class, MemorySegment.class, int.class,
                            MemorySegment.class, MemorySegment.class));
            // Process-lifetime stub (the core may call it any time until the process exits).
            stub = Linker.nativeLinker().upcallStub(target,
                    FunctionDescriptor.ofVoid(ADDRESS, JAVA_INT, ADDRESS, ADDRESS), Arena.global());
            n.setLogCallback(stub, MemorySegment.NULL);
            installed = true;
        } catch (Throwable t) {
            // A logging-bridge failure must not break streaming; fall back to dropped core logs.
            SELF.warn("could not install ggstreamlog log bridge; core logs will not be forwarded", t);
        }
    }

    /**
     * The C {@code ggsl_log_cb}. May be invoked from native background threads; never throws back
     * across the FFI boundary.
     */
    @SuppressWarnings("unused") // invoked via the upcall stub
    static void onLog(MemorySegment userData, int level, MemorySegment target, MemorySegment message) {
        try {
            String t = readCString(target, "ggstreamlog");
            String m = readCString(message, "");
            LogManager.getLogger(t).log(toLog4jLevel(level), m);
        } catch (Throwable ignore) {
            // Never propagate into native code.
        }
    }

    private static String readCString(MemorySegment seg, String fallback) {
        if (seg == null || seg.address() == 0) {
            return fallback;
        }
        return seg.reinterpret(Long.MAX_VALUE).getString(0);
    }

    private static Level toLog4jLevel(int level) {
        return switch (level) {
            case 1 -> Level.ERROR;
            case 2 -> Level.WARN;
            case 3 -> Level.INFO;
            case 4 -> Level.DEBUG;
            default -> Level.TRACE;
        };
    }
}
