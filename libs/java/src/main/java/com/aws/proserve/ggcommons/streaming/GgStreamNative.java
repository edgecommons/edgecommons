/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.streaming;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandle;
import java.nio.file.Files;
import java.nio.file.Path;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;
import static java.lang.foreign.ValueLayout.JAVA_SHORT;

/**
 * Low-level binding to the {@code ggstreamlog} C ABI ({@code include/ggstreamlog.h}) via the Java
 * Foreign Function &amp; Memory API (Panama, stable in Java 22+). Loads the native {@code cdylib}
 * once per process and exposes typed downcall wrappers; the higher-level {@link StreamService} /
 * {@link StreamHandle} build on these.
 *
 * <p>The library is located from (in order): the {@code ggstreamlog.library.path} system property
 * (absolute path to the shared library), or the platform library name on {@code java.library.path}.
 */
final class GgStreamNative {

    private static final Linker LINKER = Linker.nativeLinker();

    private static volatile GgStreamNative instance;

    private final MethodHandle open;
    private final MethodHandle streamGet;
    private final MethodHandle streamFree;
    private final MethodHandle append;
    private final MethodHandle flush;
    private final MethodHandle stats;
    private final MethodHandle shutdown;
    private final MethodHandle strFree;

    private GgStreamNative(Path libPath) {
        // Process-lifetime arena keeps the library mapped.
        Arena libArena = Arena.ofShared();
        SymbolLookup lookup = SymbolLookup.libraryLookup(libPath, libArena);

        open = down(lookup, "ggsl_open", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS));
        streamGet = down(lookup, "ggsl_stream_get",
                FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS, ADDRESS));
        streamFree = down(lookup, "ggsl_stream_free", FunctionDescriptor.ofVoid(ADDRESS));
        append = down(lookup, "ggsl_append", FunctionDescriptor.of(JAVA_INT,
                ADDRESS, ADDRESS, JAVA_SHORT, JAVA_LONG, ADDRESS, JAVA_INT, ADDRESS, ADDRESS));
        flush = down(lookup, "ggsl_flush", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
        stats = down(lookup, "ggsl_stats", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS));
        shutdown = down(lookup, "ggsl_shutdown", FunctionDescriptor.ofVoid(ADDRESS));
        strFree = down(lookup, "ggsl_str_free", FunctionDescriptor.ofVoid(ADDRESS));
    }

    private static MethodHandle down(SymbolLookup lookup, String name, FunctionDescriptor fd) {
        MemorySegment sym = lookup.find(name)
                .orElseThrow(() -> new IllegalStateException("symbol not found in ggstreamlog: " + name));
        return LINKER.downcallHandle(sym, fd);
    }

    /** The shared instance, loading the library on first use. */
    static GgStreamNative instance() {
        GgStreamNative i = instance;
        if (i == null) {
            synchronized (GgStreamNative.class) {
                i = instance;
                if (i == null) {
                    instance = i = new GgStreamNative(resolveLibraryPath());
                }
            }
        }
        return i;
    }

    private static Path resolveLibraryPath() {
        String explicit = System.getProperty("ggstreamlog.library.path");
        if (explicit != null && !explicit.isBlank()) {
            return Path.of(explicit);
        }
        String mapped = System.mapLibraryName("ggstreamlog");
        String searchPath = System.getProperty("java.library.path", "");
        for (String dir : searchPath.split(java.io.File.pathSeparator)) {
            if (dir.isBlank()) {
                continue;
            }
            Path candidate = Path.of(dir, mapped);
            if (Files.exists(candidate)) {
                return candidate;
            }
        }
        throw new IllegalStateException(
                "ggstreamlog native library not found. Set -Dggstreamlog.library.path=<path to "
                        + mapped + "> or put it on java.library.path.");
    }

    // ---- typed downcall wrappers (Throwable from invokeExact wrapped as unchecked) ----

    int open(MemorySegment configJson, MemorySegment outService, MemorySegment outErr) {
        try {
            return (int) open.invokeExact(configJson, outService, outErr);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    int streamGet(MemorySegment service, MemorySegment name, MemorySegment outStream, MemorySegment outErr) {
        try {
            return (int) streamGet.invokeExact(service, name, outStream, outErr);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    int append(MemorySegment stream, MemorySegment pk, short pkLen, long tsMs,
               MemorySegment payload, int payloadLen, MemorySegment outOffset, MemorySegment outErr) {
        try {
            return (int) append.invokeExact(stream, pk, pkLen, tsMs, payload, payloadLen, outOffset, outErr);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    int flush(MemorySegment stream, MemorySegment outErr) {
        try {
            return (int) flush.invokeExact(stream, outErr);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    int stats(MemorySegment service, MemorySegment name, MemorySegment outStats) {
        try {
            return (int) stats.invokeExact(service, name, outStats);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    void streamFree(MemorySegment stream) {
        try {
            streamFree.invokeExact(stream);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    void shutdown(MemorySegment service) {
        try {
            shutdown.invokeExact(service);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    /** Read + free a heap error string written to an {@code err} out-slot ({@code null} if unset). */
    String takeError(MemorySegment errSlot) {
        MemorySegment ptr = errSlot.get(ADDRESS, 0);
        if (ptr.address() == 0) {
            return null;
        }
        // Reinterpret the (zero-length) native pointer so we can read the NUL-terminated string.
        String msg = ptr.reinterpret(Long.MAX_VALUE).getString(0);
        try {
            strFree.invokeExact(ptr);
        } catch (Throwable t) {
            throw sneaky(t);
        }
        return msg;
    }

    private static RuntimeException sneaky(Throwable t) {
        if (t instanceof RuntimeException re) {
            return re;
        }
        if (t instanceof Error e) {
            throw e;
        }
        return new RuntimeException("ggstreamlog native call failed", t);
    }
}
