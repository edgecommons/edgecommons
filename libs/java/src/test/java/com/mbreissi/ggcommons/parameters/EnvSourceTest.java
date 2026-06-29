package com.mbreissi.ggcommons.parameters;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertTrue;

import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import org.junit.jupiter.api.Test;

/**
 * Focused unit coverage of {@link EnvSource}'s name-mapping/enumeration edges not reached by
 * {@link ParametersTest} — chiefly the {@code fromEnv} prefix-filter (an env var that does
 * <i>not</i> start with the configured prefix is excluded from {@code fetchByPath}). Uses the
 * injectable in-memory env seam (Java cannot portably mutate the process environment).
 */
class EnvSourceTest {

    private static EnvSource source(String prefix, Map<String, String> env) {
        return new EnvSource(prefix, env::get, () -> env);
    }

    @Test
    void fetchByPathSkipsVarsNotUnderPrefix() {
        Map<String, String> env = new HashMap<>();
        env.put("GGT_FROMENV_MYAPP_A", "1"); // under prefix => surfaced
        env.put("PATH", "/usr/bin");          // unrelated => fromEnv returns empty (L63 branch)
        env.put("HOME", "/home/me");          // unrelated => skipped

        List<Map.Entry<String, ParamValue>> out = source("GGT_FROMENV_", env).fetchByPath("/", true);

        assertEquals(1, out.size(), "only the prefixed var is surfaced as a parameter");
        Map.Entry<String, ParamValue> e = out.get(0);
        assertEquals("/myapp/a", e.getKey(), "_ -> / and lowercased back to a parameter name");
        assertEquals("1", new String(e.getValue().value(), StandardCharsets.UTF_8));
        assertFalse(e.getValue().secure(), "env values are always non-secure (plaintext)");
    }

    @Test
    void fetchByPathFiltersBySubtreePrefix() {
        Map<String, String> env = new HashMap<>();
        env.put("GGT_SUB_MYAPP_DB_HOST", "h");
        env.put("GGT_SUB_OTHER_X", "y");
        EnvSource src = source("GGT_SUB_", env);

        List<Map.Entry<String, ParamValue>> myapp = src.fetchByPath("/myapp", true);
        assertEquals(1, myapp.size());
        assertEquals("/myapp/db/host", myapp.get(0).getKey());
    }

    @Test
    void nameMappingNormalizesSeparatorsAndCase() {
        // Dashes and dots in the name all fold to '_' in the env-var name; leading '/' is dropped.
        Map<String, String> env = new HashMap<>();
        env.put("GGT_MAP_MY_APP_DB_HOST_NAME", "ok");
        EnvSource src = source("GGT_MAP_", env);

        // "/my-app/db.host/name" -> GGT_MAP_MY_APP_DB_HOST_NAME
        assertTrue(src.fetch("/my-app/db.host/name").isPresent());
        assertEquals("ok",
                new String(src.fetch("/my-app/db.host/name").orElseThrow().value(), StandardCharsets.UTF_8));
        assertEquals("env", src.sourceId());
    }

    @Test
    void nameMappingWithoutLeadingSlash() {
        // A name with no leading '/' still maps (the `name.startsWith("/")` false branch).
        Map<String, String> env = new HashMap<>();
        env.put("GGT_NOSLASH_FOO_BAR", "v");
        assertTrue(source("GGT_NOSLASH_", env).fetch("foo/bar").isPresent());
    }
}
