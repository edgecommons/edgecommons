/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.messaging;

import org.junit.jupiter.api.Test;

import java.util.concurrent.ExecutionException;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertTrue;

/**
 * Unit tests for {@link ReplyFuture}, a {@code CompletableFuture<Message>} that
 * carries the reply topic it is registered against.
 */
class ReplyFutureTest {

    @Test
    void constructorStoresReplyTopic() {
        ReplyFuture future = new ReplyFuture("ggcommons/reply-123");
        assertEquals("ggcommons/reply-123", future.replyTopic);
        assertFalse(future.isDone());
    }

    @Test
    void completeDeliversMessage() throws ExecutionException, InterruptedException {
        ReplyFuture future = new ReplyFuture("reply/topic");
        Message reply = Message.build("payload");

        boolean completed = future.complete(reply);

        assertTrue(completed);
        assertTrue(future.isDone());
        assertSame(reply, future.get());
        // replyTopic field remains accessible after completion
        assertEquals("reply/topic", future.replyTopic);
    }

    @Test
    void replyTopicFieldIsMutable() {
        ReplyFuture future = new ReplyFuture("initial/topic");
        future.replyTopic = "updated/topic";
        assertEquals("updated/topic", future.replyTopic);
    }
}
