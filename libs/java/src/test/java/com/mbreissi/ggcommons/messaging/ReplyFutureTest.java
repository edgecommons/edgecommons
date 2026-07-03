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

    // --- the single idempotent settle path (UNS-CANONICAL-DESIGN §5.1) ---------------------------

    @Test
    void trySettleWinsExactlyOnce() {
        ReplyFuture future = new ReplyFuture("reply/settle");
        assertFalse(future.isSettled());
        assertTrue(future.trySettle(), "first settle attempt must win");
        assertTrue(future.isSettled());
        assertFalse(future.trySettle(), "second settle attempt must lose (idempotent)");
        assertFalse(future.trySettle(), "third settle attempt must lose too");
    }

    @Test
    void settleWinnerCancelsTheAttachedDeadlineTask() {
        ReplyFuture future = new ReplyFuture("reply/timer");
        var task = new java.util.concurrent.CompletableFuture<Void>();
        // A ScheduledFuture stand-in via a real scheduler: schedule far in the future, attach,
        // settle, and assert the task got canceled.
        var scheduler = java.util.concurrent.Executors.newSingleThreadScheduledExecutor();
        try {
            var scheduled = scheduler.schedule(() -> task.complete(null), 1, java.util.concurrent.TimeUnit.HOURS);
            future.setDeadlineTask(scheduled);
            assertTrue(future.trySettle());
            assertTrue(scheduled.isCancelled(), "the settle winner must cancel the deadline timer");
        } finally {
            scheduler.shutdownNow();
        }
    }

    @Test
    void attachingTheTimerAfterSettleCancelsItImmediately() {
        // The reply can beat the scheduling call: setDeadlineTask on an already-settled future
        // must cancel the just-created timer rather than leave it to fire.
        ReplyFuture future = new ReplyFuture("reply/late-timer");
        assertTrue(future.trySettle());
        var scheduler = java.util.concurrent.Executors.newSingleThreadScheduledExecutor();
        try {
            var scheduled = scheduler.schedule(() -> { }, 1, java.util.concurrent.TimeUnit.HOURS);
            future.setDeadlineTask(scheduled);
            assertTrue(scheduled.isCancelled(), "a timer attached after settle must be canceled immediately");
        } finally {
            scheduler.shutdownNow();
        }
    }

    @Test
    void settleWithoutTimerIsSafe() {
        ReplyFuture future = new ReplyFuture("reply/no-timer");
        assertTrue(future.trySettle(), "settling with no armed deadline must not throw");
        future.setDeadlineTask(null);  // null timer is tolerated
    }
}
