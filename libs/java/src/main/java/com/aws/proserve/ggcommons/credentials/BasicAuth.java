package com.aws.proserve.ggcommons.credentials;

/** HTTP basic-auth credentials (username/password). */
public record BasicAuth(String username, String password) {
}
