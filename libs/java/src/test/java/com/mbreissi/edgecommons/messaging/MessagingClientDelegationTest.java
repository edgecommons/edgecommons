/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import com.mbreissi.edgecommons.ParsedCommandLine;
import com.google.gson.JsonObject;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;
import org.mockito.ArgumentCaptor;
import com.mbreissi.edgecommons.messaging.Qos;

import java.lang.reflect.Field;
import java.util.function.BiConsumer;

import static org.junit.jupiter.api.Assertions.*;
import static org.mockito.Mockito.*;

/**
 * Verifies that every public {@link MessagingClient} method forwards to the underlying
 * {@link MessagingProvider} with the right arguments, and that the overloads thread the
 * correct concurrency / queue-bound defaults through. The client is constructed via the
 * protected no-arg constructor and a mock provider is injected reflectively so no real
 * IPC / MQTT connection is opened. Also covers the STANDALONE-construction failure branch.
 */
class MessagingClientDelegationTest {

    private MessagingProvider provider;
    private MessagingClient client;

    @BeforeEach
    void setUp() throws Exception {
        provider = mock(MessagingProvider.class);
        client = new MessagingClient() { };
        Field f = MessagingClient.class.getDeclaredField("messagingProvider");
        f.setAccessible(true);
        f.set(client, provider);
    }

    /** A message whose toString() is safe (raw is a JsonObject), since publish* logs msg.toString(). */
    private static Message loggableMessage() {
        JsonObject body = new JsonObject();
        body.addProperty("k", "v");
        return MessageBuilder.fromObject(body);
    }

    @Test
    void publishDelegates() {
        Message msg = loggableMessage();
        client.publish("topic/a", msg);
        verify(provider).publish("topic/a", msg);
    }

    @Test
    void publishNorthboundDelegates() {
        Message msg = loggableMessage();
        client.publishNorthbound("topic/a", msg, Qos.AT_LEAST_ONCE);
        verify(provider).publishNorthbound("topic/a", msg, Qos.AT_LEAST_ONCE);
    }

    @Test
    void publishRawDelegates() {
        JsonObject obj = new JsonObject();
        obj.addProperty("k", "v");
        client.publishRaw("topic/raw", obj);
        verify(provider).publishRaw("topic/raw", obj);
    }

    @Test
    void publishNorthboundRawDelegates() {
        JsonObject obj = new JsonObject();
        client.publishNorthboundRaw("topic/raw", obj, Qos.AT_MOST_ONCE);
        verify(provider).publishNorthboundRaw("topic/raw", obj, Qos.AT_MOST_ONCE);
    }

    @Test
    void subscribeSingleArgUsesUnboundedConcurrencyAndDefaultQueue() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribe("f/+", cb);
        verify(provider).subscribe("f/+", cb, -1, MessagingClient.DEFAULT_MAX_MESSAGES);
    }

    @Test
    void subscribeWithConcurrencyUsesDefaultQueue() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribe("f/+", cb, 4);
        verify(provider).subscribe("f/+", cb, 4, MessagingClient.DEFAULT_MAX_MESSAGES);
    }

    @Test
    void subscribeFullArgsPassThrough() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribe("f/+", cb, 4, 99);
        verify(provider).subscribe("f/+", cb, 4, 99);
    }

    @Test
    void subscribeNorthboundSingleArgDefaults() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE);
        verify(provider).subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE, -1, MessagingClient.DEFAULT_MAX_MESSAGES);
    }

    @Test
    void subscribeNorthboundWithConcurrencyDefaultQueue() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE, 7);
        verify(provider).subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE, 7, MessagingClient.DEFAULT_MAX_MESSAGES);
    }

    @Test
    void subscribeNorthboundFullArgsPassThrough() {
        BiConsumer<String, Message> cb = (t, m) -> { };
        client.subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE, 7, 42);
        verify(provider).subscribeNorthbound("f/+", cb, Qos.AT_LEAST_ONCE, 7, 42);
    }

    @Test
    void requestDelegatesAndReturnsFuture() {
        Message req = MessageBuilder.fromObject("q");
        ReplyFuture rf = new ReplyFuture("reply/x");
        when(provider.request("topic/req", req)).thenReturn(rf);
        assertSame(rf, client.request("topic/req", req));
    }

    @Test
    void requestNorthboundDelegatesAndReturnsFuture() {
        Message req = MessageBuilder.fromObject("q");
        ReplyFuture rf = new ReplyFuture("reply/x");
        when(provider.requestNorthbound("topic/req", req)).thenReturn(rf);
        assertSame(rf, client.requestNorthbound("topic/req", req));
    }

    @Test
    void requestWithTimeoutDelegatesAndReturnsFuture() {
        Message req = MessageBuilder.fromObject("q");
        ReplyFuture rf = new ReplyFuture("reply/x");
        java.time.Duration t = java.time.Duration.ofSeconds(5);
        when(provider.request("topic/req", req, t)).thenReturn(rf);
        assertSame(rf, client.request("topic/req", req, t));
        verify(provider).request("topic/req", req, t);
    }

    @Test
    void requestNorthboundWithTimeoutDelegatesAndReturnsFuture() {
        Message req = MessageBuilder.fromObject("q");
        ReplyFuture rf = new ReplyFuture("reply/x");
        when(provider.requestNorthbound("topic/req", req, java.time.Duration.ZERO)).thenReturn(rf);
        assertSame(rf, client.requestNorthbound("topic/req", req, java.time.Duration.ZERO));
    }

    @Test
    void setDefaultRequestTimeoutDelegates() {
        client.setDefaultRequestTimeout(java.time.Duration.ofSeconds(7));
        verify(provider).setDefaultRequestTimeout(java.time.Duration.ofSeconds(7));
    }

    @Test
    void getDefaultRequestTimeoutDelegates() {
        when(provider.getDefaultRequestTimeout()).thenReturn(java.time.Duration.ofSeconds(9));
        assertEquals(java.time.Duration.ofSeconds(9), client.getDefaultRequestTimeout());
    }

    @Test
    void requestTimeoutAccessorsAreSafeWhenProviderNull() throws Exception {
        // The late-bind call must be a safe no-op on a provider-less client (mock/test subclass).
        MessagingClient bare = new MessagingClient() { };
        Field f = MessagingClient.class.getDeclaredField("messagingProvider");
        f.setAccessible(true);
        f.set(bare, null);
        assertDoesNotThrow(() -> bare.setDefaultRequestTimeout(java.time.Duration.ofSeconds(3)));
        assertNull(bare.getDefaultRequestTimeout());
    }

    @Test
    void cancelRequestDelegates() {
        ReplyFuture rf = new ReplyFuture("reply/x");
        client.cancelRequest(rf);
        verify(provider).cancelRequest(rf);
    }

    @Test
    void cancelRequestNorthboundDelegates() {
        ReplyFuture rf = new ReplyFuture("reply/x");
        client.cancelRequestNorthbound(rf);
        verify(provider).cancelRequestNorthbound(rf);
    }

    @Test
    void replyDelegates() {
        // reply() logs request.getHeader().getReplyTo() and reply.toString(); give the request a
        // header and a loggable reply body.
        Message request = MessageBuilder.create("Req", "1.0")
                .withConfig(tagOnlyConfig())
                .build();
        request.makeRequest("reply/here");
        Message reply = loggableMessage();
        client.reply(request, reply);
        verify(provider).reply(request, reply);
    }

    @Test
    void replyNorthboundDelegates() {
        Message request = loggableMessage();
        Message reply = loggableMessage();
        client.replyNorthbound(request, reply);
        verify(provider).replyNorthbound(request, reply);
    }

    @Test
    void unsubscribeDelegates() {
        client.unsubscribe("f/+");
        verify(provider).unsubscribe("f/+");
    }

    @Test
    void unsubscribeNorthboundDelegates() {
        client.unsubscribeNorthbound("f/+");
        verify(provider).unsubscribeNorthbound("f/+");
    }

    @Test
    void getNativeLocalClientDelegates() {
        Object nativeObj = new Object();
        when(provider.getNativeLocalClient()).thenReturn(nativeObj);
        assertSame(nativeObj, client.getNativeLocalClient());
    }

    @Test
    void getNativeNorthboundClientDelegates() {
        Object nativeObj = new Object();
        when(provider.getNativeNorthboundClient()).thenReturn(nativeObj);
        assertSame(nativeObj, client.getNativeNorthboundClient());
    }

    @Test
    void connectedDelegatesToProvider() {
        when(provider.connected()).thenReturn(true);
        assertTrue(client.connected());
        when(provider.connected()).thenReturn(false);
        assertFalse(client.connected());
    }

    @Test
    void connectedIsFalseWhenProviderNull() throws Exception {
        MessagingClient bare = new MessagingClient() { };
        Field f = MessagingClient.class.getDeclaredField("messagingProvider");
        f.setAccessible(true);
        f.set(bare, null);
        assertFalse(bare.connected(), "no provider wired -> not connected (treated as not-ready)");
    }

    @Test
    void closeDelegatesWhenProviderPresent() {
        client.close();
        verify(provider).close();
    }

    @Test
    void closeIsSafeWhenProviderNull() throws Exception {
        MessagingClient bare = new MessagingClient() { };
        Field f = MessagingClient.class.getDeclaredField("messagingProvider");
        f.setAccessible(true);
        f.set(bare, null);
        // Must not throw when the provider was never wired up.
        assertDoesNotThrow(bare::close);
    }

    @Test
    void topicMatchesFilterStaticDelegatesToProvider() {
        assertTrue(MessagingClient.topicMatchesFilter("sport/#", "sport/tennis"));
        assertFalse(MessagingClient.topicMatchesFilter("a", "b"));
    }

    @Test
    void standaloneConstructionWithBadPathThrowsRuntimeException() {
        ParsedCommandLine pcl = new ParsedCommandLine();
        pcl.transport = com.mbreissi.edgecommons.platform.Transport.MQTT;
        pcl.standaloneConfigPath = "/definitely/not/a/real/path/standalone.json";
        pcl.thingName = "thing-1";

        RuntimeException ex = assertThrows(RuntimeException.class,
                () -> new MessagingClient(pcl, false));
        assertTrue(ex.getMessage().contains("standalone messaging configuration"));
    }

    /** Minimal ConfigManager mock that returns no tag config and a thing name. */
    private static com.mbreissi.edgecommons.config.ConfigManager tagOnlyConfig() {
        com.mbreissi.edgecommons.config.ConfigManager cm =
                mock(com.mbreissi.edgecommons.config.ConfigManager.class);
        when(cm.getTagConfig()).thenReturn(null);
        when(cm.getThingName()).thenReturn("thing-1");
        return cm;
    }
}
