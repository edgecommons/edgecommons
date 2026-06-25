package com.breissinger.ggcommons.parameters;

import java.nio.charset.StandardCharsets;
import java.util.AbstractMap;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.Function;
import java.util.function.Supplier;

/**
 * Reads parameters from environment variables under a prefix. A name {@code /myapp/db/host} maps to
 * the env var {@code <PREFIX>MYAPP_DB_HOST} and back. Values are treated as non-secure (env is
 * plaintext). Mirrors the Rust {@code EnvSource}.
 *
 * <p><b>Test seam (idiomatic-Java adaptation):</b> Java cannot portably set process environment
 * variables, so this source reads env through an injectable {@code lookup}/{@code allVars} pair that
 * defaults to {@link System#getenv()}. Production code uses the default; tests pass an in-memory map.
 * This is the only public surface added beyond the Rust reference, and it changes no semantics.
 */
public final class EnvSource implements ParameterSource {
    private final String prefix;
    private final Function<String, String> lookup;
    private final Supplier<Map<String, String>> allVars;

    /** New source reading vars under {@code prefix} (e.g. {@code "GG_PARAM_"}) from the process environment. */
    public EnvSource(String prefix) {
        this(prefix, System::getenv, System::getenv);
    }

    /**
     * New source reading vars under {@code prefix} via an injected environment (the test seam).
     *
     * @param prefix  the env-var prefix
     * @param lookup  resolves a single env var by name (e.g. {@code System::getenv})
     * @param allVars supplies the full env map (for {@code fetchByPath} enumeration)
     */
    public EnvSource(String prefix, Function<String, String> lookup, Supplier<Map<String, String>> allVars) {
        this.prefix = prefix;
        this.lookup = lookup;
        this.allVars = allVars;
    }

    /** Map a parameter name to its env-var name. */
    private String toEnv(String name) {
        StringBuilder body = new StringBuilder();
        String trimmed = name.startsWith("/") ? name.substring(1) : name;
        for (int i = 0; i < trimmed.length(); i++) {
            char c = trimmed.charAt(i);
            if (c == '/' || c == '-' || c == '.') {
                body.append('_');
            } else {
                body.append(Character.toUpperCase(c));
            }
        }
        return prefix + body;
    }

    /** Map an env-var name back to a parameter name (lossy: {@code _} -> {@code /}). */
    private Optional<String> fromEnv(String var) {
        if (!var.startsWith(prefix)) {
            return Optional.empty();
        }
        String rest = var.substring(prefix.length());
        return Optional.of("/" + rest.toLowerCase().replace('_', '/'));
    }

    @Override
    public Optional<ParamValue> fetch(String name) {
        String v = lookup.apply(toEnv(name));
        return v == null ? Optional.empty() : Optional.of(ParamValue.plain(v.getBytes(StandardCharsets.UTF_8)));
    }

    @Override
    public List<Map.Entry<String, ParamValue>> fetchByPath(String path, boolean recursive) {
        List<Map.Entry<String, ParamValue>> out = new ArrayList<>();
        for (Map.Entry<String, String> e : allVars.get().entrySet()) {
            Optional<String> name = fromEnv(e.getKey());
            if (name.isPresent() && name.get().startsWith(path)) {
                out.add(new AbstractMap.SimpleImmutableEntry<>(
                        name.get(), ParamValue.plain(e.getValue().getBytes(StandardCharsets.UTF_8))));
            }
        }
        return out;
    }

    @Override
    public String sourceId() {
        return "env";
    }
}
