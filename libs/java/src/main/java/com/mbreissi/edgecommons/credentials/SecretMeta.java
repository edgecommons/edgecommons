package com.mbreissi.edgecommons.credentials;

import java.util.Map;

/** Metadata for a secret version — safe to log/list (no value). */
public record SecretMeta(
        String name,
        String version,
        long createdMs,
        Long ttlSecs,
        String source,
        Map<String, String> labels) {
}
