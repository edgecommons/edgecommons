package com.mbreissi.ggcommons.credentials;

import java.util.Map;

/** A secret value fetched from the central source. */
public record CentralSecret(byte[] bytes, String centralVersionId, Map<String, String> labels) {
}
