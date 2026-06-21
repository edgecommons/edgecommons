package com.aws.proserve.ggcommons.credentials;

/** A TLS bundle (PEM strings). */
public record TlsBundle(String certPem, String keyPem, String caPem) {
}
