package com.breissinger.ggcommons.credentials;

/** A TLS bundle (PEM strings). */
public record TlsBundle(String certPem, String keyPem, String caPem) {
}
