/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.streaming;

import java.io.InputStream;
import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandle;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.Locale;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;
import static java.lang.foreign.ValueLayout.JAVA_SHORT;

/**
 * Low-level binding to the {@code edgestreamlog} C ABI ({@code include/edgestreamlog.h}) via the Java
 * Foreign Function &amp; Memory API (Panama, stable in Java 22+). Loads the native {@code cdylib}
 * once per process and exposes typed downcall wrappers; the higher-level {@link StreamService} /
 * {@link StreamHandle} build on these.
 *
 * <p>The library is located from (in order): the {@code edgestreamlog.library.path} system property
 * (absolute path to the shared library), or the platform library name on {@code java.library.path}.
 */
final class EdgeStreamNative {

    private static final Linker LINKER = Linker.nativeLinker();

    private static volatile EdgeStreamNative instance;

    private final MethodHandle open;
    private final MethodHandle streamGet;
    private final MethodHandle streamFree;
    private final MethodHandle append;
    private final MethodHandle flush;
    private final MethodHandle stats;
    private final MethodHandle shutdown;
    private final MethodHandle strFree;
    private final MethodHandle setLogCallback;
    private final MethodHandle setSinkCallback;

    private EdgeStreamNative(Path libPath) {
        // Process-lifetime arena keeps the library mapped.
        Arena libArena = Arena.ofShared();
        SymbolLookup lookup = SymbolLookup.libraryLookup(libPath, libArena);

        open = down(lookup, "esl_open", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS));
        streamGet = down(lookup, "esl_stream_get",
                FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS, ADDRESS));
        streamFree = down(lookup, "esl_stream_free", FunctionDescriptor.ofVoid(ADDRESS));
        append = down(lookup, "esl_append", FunctionDescriptor.of(JAVA_INT,
                ADDRESS, ADDRESS, JAVA_SHORT, JAVA_LONG, ADDRESS, JAVA_INT, ADDRESS, ADDRESS));
        flush = down(lookup, "esl_flush", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
        stats = down(lookup, "esl_stats", FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS, ADDRESS));
        shutdown = down(lookup, "esl_shutdown", FunctionDescriptor.ofVoid(ADDRESS));
        strFree = down(lookup, "esl_str_free", FunctionDescriptor.ofVoid(ADDRESS));
        setLogCallback = down(lookup, "esl_set_log_callback",
                FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));
        setSinkCallback = down(lookup, "esl_set_sink_callback",
                FunctionDescriptor.of(JAVA_INT, ADDRESS, ADDRESS));

        // Route the core's log events into the host logger (log4j2) for the rest of the process.
        NativeLogBridge.install(this);
    }

    private static MethodHandle down(SymbolLookup lookup, String name, FunctionDescriptor fd) {
        MemorySegment sym = lookup.find(name)
                .orElseThrow(() -> new IllegalStateException("symbol not found in edgestreamlog: " + name));
        return LINKER.downcallHandle(sym, fd);
    }

    /** The shared instance, loading the library on first use. */
    static EdgeStreamNative instance() {
        EdgeStreamNative i = instance;
        if (i == null) {
            synchronized (EdgeStreamNative.class) {
                i = instance;
                if (i == null) {
                    instance = i = new EdgeStreamNative(resolveLibraryPath());
                }
            }
        }
        return i;
    }

    private static Path resolveLibraryPath() {
        String explicit = System.getProperty("edgestreamlog.library.path");
        if (explicit != null && !explicit.isBlank()) {
            return Path.of(explicit);
        }
        String mapped = System.mapLibraryName("edgestreamlog");
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
        // Bundled in the jar at /native/<os>-<arch>/<mapped> → extract to a temp file.
        Path extracted = extractBundled(mapped);
        if (extracted != null) {
            return extracted;
        }
        throw new IllegalStateException(
                "edgestreamlog native library not found. Set -Dedgestreamlog.library.path=<path to "
                        + mapped + ">, put it on java.library.path, or bundle it at "
                        + "/native/" + osArch() + "/" + mapped + " on the classpath.");
    }

    /** Extract the platform library from the classpath ({@code /native/<os>-<arch>/<name>}) if present. */
    private static Path extractBundled(String mapped) {
        String resource = "/native/" + osArch() + "/" + mapped;
        try (InputStream in = EdgeStreamNative.class.getResourceAsStream(resource)) {
            if (in == null) {
                return null;
            }
            String suffix = mapped.contains(".") ? mapped.substring(mapped.lastIndexOf('.')) : ".bin";
            Path tmp = Files.createTempFile("edgestreamlog", suffix);
            Files.copy(in, tmp, StandardCopyOption.REPLACE_EXISTING);
            tmp.toFile().deleteOnExit();
            return tmp;
        } catch (Exception e) {
            return null;
        }
    }

    /** Platform tag for the bundled-resource path, e.g. {@code linux-x86_64}, {@code windows-x86_64}. */
    private static String osArch() {
        String os = System.getProperty("os.name", "").toLowerCase(Locale.ROOT);
        String arch = System.getProperty("os.arch", "").toLowerCase(Locale.ROOT);
        String osTag = os.contains("win") ? "windows" : os.contains("mac") || os.contains("darwin") ? "darwin" : "linux";
        String archTag = switch (arch) {
            case "amd64", "x86_64" -> "x86_64";
            case "aarch64", "arm64" -> "aarch64";
            default -> arch;
        };
        return osTag + "-" + archTag;
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

    int setLogCallback(MemorySegment cb, MemorySegment userData) {
        try {
            return (int) setLogCallback.invokeExact(cb, userData);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    int setSinkCallback(MemorySegment cb, MemorySegment userData) {
        try {
            return (int) setSinkCallback.invokeExact(cb, userData);
        } catch (Throwable t) {
            throw sneaky(t);
        }
    }

    /** The shared native linker, for building upcall stubs in {@link SinkBridge}. */
    static Linker linker() {
        return LINKER;
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
        return new RuntimeException("edgestreamlog native call failed", t);
    }
}
