/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.messaging.providers.standalone;

import org.junit.jupiter.api.Test;

import java.io.IOException;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Direct unit tests for the minimal ASN.1/DER decoder ({@code Asn1Object} + {@code DerParser})
 * used by {@link PrivateKeyReader} for PKCS#1 keys.
 */
class Asn1ObjectTest {

    @Test
    void integerObject() throws Exception {
        Asn1Object o = new Asn1Object(DerParser.INTEGER, 1, new byte[]{0x07});
        assertEquals(DerParser.INTEGER, o.getType());
        assertEquals(1, o.getLength());
        assertArrayEquals(new byte[]{0x07}, o.getValue());
        assertFalse(o.isConstructed());
        assertEquals(BigInteger.valueOf(7), o.getInteger());
    }

    @Test
    void stringObjectsDecodeAcrossEncodings() throws Exception {
        int[] latin1Types = {
                DerParser.NUMERIC_STRING, DerParser.PRINTABLE_STRING, DerParser.VIDEOTEX_STRING,
                DerParser.IA5_STRING, DerParser.GRAPHIC_STRING, DerParser.ISO646_STRING, DerParser.GENERAL_STRING
        };
        for (int type : latin1Types) {
            Asn1Object o = new Asn1Object(type, 2, "hi".getBytes(StandardCharsets.ISO_8859_1));
            assertEquals("hi", o.getString());
        }
        assertEquals("hi", new Asn1Object(DerParser.UTF8_STRING, 2, "hi".getBytes(StandardCharsets.UTF_8)).getString());
        assertEquals("hi", new Asn1Object(DerParser.BMP_STRING, 4, "hi".getBytes(StandardCharsets.UTF_16BE)).getString());
    }

    @Test
    void getStringRejectsNonStringAndUcs4() {
        assertThrows(IOException.class, () -> new Asn1Object(DerParser.INTEGER, 1, new byte[]{1}).getString());
        assertThrows(IOException.class, () -> new Asn1Object(DerParser.UNIVERSAL_STRING, 1, new byte[]{1}).getString());
    }

    @Test
    void getIntegerRejectsNonInteger() {
        assertThrows(IOException.class,
                () -> new Asn1Object(DerParser.PRINTABLE_STRING, 1, new byte[]{'a'}).getInteger());
    }

    @Test
    void constructedSequenceParsesNestedInteger() throws Exception {
        // SEQUENCE (0x30) whose content is one INTEGER TLV: 02 01 09
        byte[] content = new byte[]{0x02, 0x01, 0x09};
        Asn1Object seq = new Asn1Object(DerParser.CONSTRUCTED | DerParser.SEQUENCE, content.length, content);
        assertTrue(seq.isConstructed());

        DerParser parser = seq.getParser();
        Asn1Object inner = parser.read();
        assertEquals(BigInteger.valueOf(9), inner.getInteger());
    }

    @Test
    void getParserRejectsPrimitive() {
        assertThrows(IOException.class, () -> new Asn1Object(DerParser.INTEGER, 1, new byte[]{1}).getParser());
    }

    @Test
    void derParserReadsLongFormLength() throws Exception {
        // INTEGER with long-form length: 0x02, 0x81 (1 length octet follows), 0x02 (len=2), value 0x01 0x02
        byte[] der = new byte[]{0x02, (byte) 0x81, 0x02, 0x01, 0x02};
        DerParser parser = new DerParser(der);
        Asn1Object o = parser.read();
        assertEquals(2, o.getLength());
        assertEquals(new BigInteger(new byte[]{0x01, 0x02}), o.getInteger());
    }
}
