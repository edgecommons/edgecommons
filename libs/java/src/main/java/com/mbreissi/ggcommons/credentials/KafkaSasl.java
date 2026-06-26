package com.mbreissi.ggcommons.credentials;

/** Kafka SASL credentials (mechanism defaults to PLAIN when absent). */
public record KafkaSasl(String mechanism, String username, String password) {
}
