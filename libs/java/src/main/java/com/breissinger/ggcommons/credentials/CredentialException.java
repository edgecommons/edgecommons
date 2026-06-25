package com.breissinger.ggcommons.credentials;

/**
 * Any vault/credential failure (bad key, tamper, I/O, unimplemented provider).
 * Messages never include secret or key material.
 */
public class CredentialException extends RuntimeException {
    public CredentialException(String message) {
        super(message);
    }

    public CredentialException(String message, Throwable cause) {
        super(message, cause);
    }
}
