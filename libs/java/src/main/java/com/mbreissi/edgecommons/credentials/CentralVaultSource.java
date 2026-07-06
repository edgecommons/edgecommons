package com.mbreissi.edgecommons.credentials;

import java.util.Optional;

/** The upstream source a vault syncs from (pull-only). */
public interface CentralVaultSource {
    /** Fetch the current value of {@code name}, or empty if it does not exist upstream. */
    Optional<CentralSecret> fetch(String name);
}
