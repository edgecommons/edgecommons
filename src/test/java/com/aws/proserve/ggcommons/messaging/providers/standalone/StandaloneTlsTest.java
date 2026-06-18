/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging.providers.standalone;

import com.aws.proserve.ggcommons.messaging.MessagingConfiguration;
import com.google.gson.Gson;
import org.junit.jupiter.api.Test;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.io.File;
import java.security.PrivateKey;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

/**
 * Unit tests for the standalone TLS/credential plumbing: PKCS#1 and PKCS#8 private-key parsing
 * ({@link PrivateKeyReader} + DerParser/Asn1Object) and the SSL builders ({@code createSslContext},
 * {@code getSocketFactory}) for server-only and mutual TLS. Uses committed self-signed test fixtures.
 */
class StandaloneTlsTest {

    private String path(String resource) throws Exception {
        return new File(getClass().getResource(resource).toURI()).getAbsolutePath();
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
        PrivateKey key = PrivateKeyReader.getPrivateKey(path("/certs/client.pkcs1.key"));
        assertNotNull(key);
        assertEquals("RSA", key.getAlgorithm());
    }

    @Test
    void readsPkcs8PrivateKey() throws Exception {
        // PKCS#8 ("BEGIN PRIVATE KEY") exercises the PKCS8EncodedKeySpec path.
        PrivateKey key = PrivateKeyReader.getPrivateKey(path("/certs/client.pkcs8.key"));
        assertNotNull(key);
        assertEquals("RSA", key.getAlgorithm());
    }

    @Test
    void createSslContextMutualTls() throws Exception {
        SSLContext ctx = StandaloneMessagingProvider.createSslContext(
                creds(path("/certs/ca.crt"), path("/certs/client.crt"), path("/certs/client.pkcs1.key")));
        assertNotNull(ctx);
        assertNotNull(ctx.getSocketFactory());
    }

    @Test
    void createSslContextServerOnlyTls() throws Exception {
        // CA only, no client cert/key => server-only TLS (no key managers).
        SSLContext ctx = StandaloneMessagingProvider.createSslContext(creds(path("/certs/ca.crt"), null, null));
        assertNotNull(ctx);
        assertNotNull(ctx.getSocketFactory());
    }

    @Test
    void getSocketFactoryBuildsMutualTls() throws Exception {
        SSLSocketFactory sf = StandaloneMessagingProvider.getSocketFactory(
                path("/certs/ca.crt"), path("/certs/client.crt"), path("/certs/client.pkcs1.key"));
        assertNotNull(sf);
    }
}
