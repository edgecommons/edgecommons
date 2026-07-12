/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging.providers.standalone;

import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.GeneralSecurityException;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.interfaces.RSAPrivateCrtKey;
import java.util.Base64;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link PrivateKeyReader}: the PEM reader used by the STANDALONE (MQTT) provider
 * to load a client private key for mutual TLS.
 *
 * <p>The fixtures are generated in-process (no committed key material): an RSA key pair is emitted
 * both as PKCS#8 ({@code BEGIN PRIVATE KEY}) and as PKCS#1 ({@code BEGIN RSA PRIVATE KEY}, DER-encoded
 * here from its CRT parameters), and an EC key pair as PKCS#8.
 */
class PrivateKeyReaderTest {

    private static KeyPair rsaKeyPair() throws GeneralSecurityException {
        KeyPairGenerator gen = KeyPairGenerator.getInstance("RSA");
        gen.initialize(2048);
        return gen.generateKeyPair();
    }

    private static KeyPair ecKeyPair() throws GeneralSecurityException {
        KeyPairGenerator gen = KeyPairGenerator.getInstance("EC");
        gen.initialize(256);
        return gen.generateKeyPair();
    }

    /** Wraps DER bytes in a PEM block with the given label, e.g. {@code RSA PRIVATE KEY}. */
    private static String pem(String label, byte[] der) {
        String body = Base64.getMimeEncoder(64, new byte[]{'\n'}).encodeToString(der);
        return "-----BEGIN " + label + "-----\n" + body + "\n-----END " + label + "-----\n";
    }

    /** DER TLV. */
    private static byte[] tlv(int tag, byte[] content) {
        ByteArrayOutputStream out = new ByteArrayOutputStream();
        out.write(tag);
        int len = content.length;
        if (len < 0x80) {
            out.write(len);
        } else {
            byte[] lenBytes = BigInteger.valueOf(len).toByteArray();
            int off = lenBytes[0] == 0 ? 1 : 0;
            int n = lenBytes.length - off;
            out.write(0x80 | n);
            out.write(lenBytes, off, n);
        }
        out.writeBytes(content);
        return out.toByteArray();
    }

    private static byte[] derInt(BigInteger value) {
        return tlv(0x02, value.toByteArray());
    }

    /** Encodes an RSA private key in PKCS#1 (RSAPrivateKey SEQUENCE of nine INTEGERs). */
    private static byte[] pkcs1(RSAPrivateCrtKey key) {
        ByteArrayOutputStream seq = new ByteArrayOutputStream();
        seq.writeBytes(derInt(BigInteger.ZERO)); // version
        seq.writeBytes(derInt(key.getModulus()));
        seq.writeBytes(derInt(key.getPublicExponent()));
        seq.writeBytes(derInt(key.getPrivateExponent()));
        seq.writeBytes(derInt(key.getPrimeP()));
        seq.writeBytes(derInt(key.getPrimeQ()));
        seq.writeBytes(derInt(key.getPrimeExponentP()));
        seq.writeBytes(derInt(key.getPrimeExponentQ()));
        seq.writeBytes(derInt(key.getCrtCoefficient()));
        return tlv(0x30, seq.toByteArray()); // CONSTRUCTED | SEQUENCE
    }

    private static ByteArrayInputStream stream(String text) {
        return new ByteArrayInputStream(text.getBytes(StandardCharsets.UTF_8));
    }

    @Test
    void readsPkcs8RsaKeyFromStream() throws Exception {
        KeyPair pair = rsaKeyPair();
        String text = pem("PRIVATE KEY", pair.getPrivate().getEncoded());

        PrivateKey key = PrivateKeyReader.getPrivateKey(stream(text), null);

        assertEquals("RSA", key.getAlgorithm());
        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void readsPkcs1RsaKeyFromStream() throws Exception {
        KeyPair pair = rsaKeyPair();
        String text = pem("RSA PRIVATE KEY", pkcs1((RSAPrivateCrtKey) pair.getPrivate()));

        PrivateKey key = PrivateKeyReader.getPrivateKey(stream(text), "RSA");

        assertEquals("RSA", key.getAlgorithm());
        // The PKCS#1 CRT parameters must round-trip to the very same key.
        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void readsPkcs8EcKeyFromStream() throws Exception {
        KeyPair pair = ecKeyPair();
        String text = pem("PRIVATE KEY", pair.getPrivate().getEncoded());

        PrivateKey key = PrivateKeyReader.getPrivateKey(stream(text), "EC");

        assertEquals("EC", key.getAlgorithm());
        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void ignoresLeadingAndTrailingNoise() throws Exception {
        KeyPair pair = rsaKeyPair();
        String text = "# a comment\nBag Attributes: none\n"
                + pem("PRIVATE KEY", pair.getPrivate().getEncoded())
                + "trailing noise\n";

        PrivateKey key = PrivateKeyReader.getPrivateKey(stream(text), null);

        assertNotNull(key);
        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void readsKeyFromFileWithDefaultRsaAlgorithm(@TempDir Path dir) throws Exception {
        KeyPair pair = rsaKeyPair();
        Path file = dir.resolve("client.pkcs8.key");
        Files.writeString(file, pem("PRIVATE KEY", pair.getPrivate().getEncoded()));

        PrivateKey key = PrivateKeyReader.getPrivateKey(file.toString());

        assertEquals("RSA", key.getAlgorithm());
        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void readsKeyFromFileWithExplicitAlgorithm(@TempDir Path dir) throws Exception {
        KeyPair pair = ecKeyPair();
        Path file = dir.resolve("client.ec.key");
        Files.writeString(file, pem("PRIVATE KEY", pair.getPrivate().getEncoded()));

        PrivateKey key = PrivateKeyReader.getPrivateKey(file.toString(), "EC");

        assertEquals("EC", key.getAlgorithm());
    }

    @Test
    void readsPkcs1KeyFromFile(@TempDir Path dir) throws Exception {
        KeyPair pair = rsaKeyPair();
        Path file = dir.resolve("client.pkcs1.key");
        Files.writeString(file, pem("RSA PRIVATE KEY", pkcs1((RSAPrivateCrtKey) pair.getPrivate())));

        PrivateKey key = PrivateKeyReader.getPrivateKey(file.toString());

        assertArrayEquals(pair.getPrivate().getEncoded(), key.getEncoded());
    }

    @Test
    void rejectsFileLongerThanTheLineBudget() {
        String text = "noise\n".repeat(120);

        IOException error = assertThrows(IOException.class, () -> PrivateKeyReader.getPrivateKey(stream(text), null));

        assertTrue(error.getMessage().contains("maximum number of lines"), error.getMessage());
    }

    @Test
    void rejectsPkcs1BodyThatIsNotAnAsn1Sequence() {
        // A well-formed PEM envelope whose DER payload is a bare INTEGER, not a SEQUENCE.
        String text = pem("RSA PRIVATE KEY", derInt(BigInteger.valueOf(7)));

        IOException error = assertThrows(IOException.class, () -> PrivateKeyReader.getPrivateKey(stream(text), null));

        assertTrue(error.getMessage().contains("not a sequence"), error.getMessage());
    }

    @Test
    void rejectsGarbageKeyMaterial() {
        String text = pem("PRIVATE KEY", new byte[]{0x01, 0x02, 0x03, 0x04});

        assertThrows(GeneralSecurityException.class, () -> PrivateKeyReader.getPrivateKey(stream(text), null));
    }

    @Test
    void missingFileSurfacesAsIoException() {
        assertThrows(IOException.class, () -> PrivateKeyReader.getPrivateKey("no-such-key.pem"));
        assertThrows(IOException.class, () -> PrivateKeyReader.getPrivateKey("no-such-key.pem", "RSA"));
    }
}
