/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging.providers.standalone;

import com.breissinger.ggcommons.messaging.MessagingConfiguration;
import com.google.gson.Gson;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.io.File;
import java.net.URL;
import java.security.PrivateKey;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

/**
 * Unit tests for the standalone TLS/credential plumbing: PKCS#1 and PKCS#8 private-key parsing
 * ({@link PrivateKeyReader} + DerParser/Asn1Object) and the SSL builders ({@code createSslContext},
 * {@code getSocketFactory}) for server-only and mutual TLS.
 *
 * <p>Uses self-signed test fixtures under {@code src/test/resources/certs/} — throwaway
 * RSA key material generated for this suite (never real IoT device credentials). If those
 * resources are absent the tests <b>self-skip</b> (JUnit {@link org.junit.jupiter.api.Assumptions
 * assumptions}) rather than error, so a checkout missing the fixtures still keeps
 * {@code mvn verify} green.
 */
class StandaloneTlsTest {

    /**
     * Resolves a classpath test resource to an absolute filesystem path, or {@code null} if the
     * resource is not present (so callers can {@code assumeTrue} and skip cleanly).
     */
    private String path(String resource) throws Exception {
        URL url = getClass().getResource(resource);
        if (url == null) {
            return null;
        }
        return new File(url.toURI()).getAbsolutePath();
    }

    private MessagingConfiguration.CredentialsConfig creds(String ca, String cert, String key) {
        Gson gson = new Gson();
        StringBuilder sb = new StringBuilder("{");
        boolean first = true;
        if (ca != null)   { sb.append("\"caPath\":").append(gson.toJson(ca)); first = false; }
        if (cert != null) { if (!first) sb.append(","); sb.append("\"certPath\":").append(gson.toJson(cert)); first = false; }
        if (key != null)  { if (!first) sb.append(","); sb.append("\"keyPath\":").append(gson.toJson(key)); }
        sb.append("}");
        return gson.fromJson(sb.toString(), MessagingConfiguration.CredentialsConfig.class);
    }

    @Test
    void readsPkcs1RsaPrivateKey() throws Exception {
        // PKCS#1 ("BEGIN RSA PRIVATE KEY") exercises the custom DerParser / Asn1Object path.
        String keyPath = path("/certs/client.pkcs1.key");
        assumeTrue(keyPath != null, "missing test fixture /certs/client.pkcs1.key");
        PrivateKey key = PrivateKeyReader.getPrivateKey(keyPath);
        assertNotNull(key);
        assertEquals("RSA", key.getAlgorithm());
    }

    @Test
    void readsPkcs8PrivateKey() throws Exception {
        // PKCS#8 ("BEGIN PRIVATE KEY") exercises the PKCS8EncodedKeySpec path.
        String keyPath = path("/certs/client.pkcs8.key");
        assumeTrue(keyPath != null, "missing test fixture /certs/client.pkcs8.key");
        PrivateKey key = PrivateKeyReader.getPrivateKey(keyPath);
        assertNotNull(key);
        assertEquals("RSA", key.getAlgorithm());
    }

    @Test
    void createSslContextMutualTls() throws Exception {
        String ca = path("/certs/ca.crt");
        String cert = path("/certs/client.crt");
        String key = path("/certs/client.pkcs1.key");
        assumeTrue(ca != null && cert != null && key != null, "missing mutual-TLS test fixtures under /certs");
        SSLContext ctx = StandaloneMessagingProvider.createSslContext(creds(ca, cert, key));
        assertNotNull(ctx);
        assertNotNull(ctx.getSocketFactory());
    }

    @Test
    void createSslContextServerOnlyTls() throws Exception {
        // CA only, no client cert/key => server-only TLS (no key managers).
        String ca = path("/certs/ca.crt");
        assumeTrue(ca != null, "missing test fixture /certs/ca.crt");
        SSLContext ctx = StandaloneMessagingProvider.createSslContext(creds(ca, null, null));
        assertNotNull(ctx);
        assertNotNull(ctx.getSocketFactory());
    }

    @Test
    void getSocketFactoryBuildsMutualTls() throws Exception {
        String ca = path("/certs/ca.crt");
        String cert = path("/certs/client.crt");
        String key = path("/certs/client.pkcs1.key");
        assumeTrue(ca != null && cert != null && key != null, "missing mutual-TLS test fixtures under /certs");
        SSLSocketFactory sf = StandaloneMessagingProvider.getSocketFactory(ca, cert, key);
        assertNotNull(sf);
    }
}
