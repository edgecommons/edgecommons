/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.messaging;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * The future returned by {@code request()}: a {@code CompletableFuture<Message>} carrying the
 * ephemeral reply topic the request is registered against, plus the request's single idempotent
 * <em>settle</em> path (UNS-CANONICAL-DESIGN §5.1): reply-arrival, the framework deadline and
 * {@code cancelRequest} all race through {@link #trySettle()}; exactly one wins and performs the
 * cleanup (unsubscribe + pending-entry removal) and the completion; the losers no-op.
 */
public class ReplyFuture extends CompletableFuture<Message>
{
    public String replyTopic;

    /** The per-request settle flag (§5.1): CAS'd by reply-arrival / deadline / cancel. */
    private final AtomicBoolean settled = new AtomicBoolean(false);

    /** The framework-owned deadline timer, when one was armed; canceled by the settle winner. */
    private volatile ScheduledFuture<?> deadlineTask;

    public ReplyFuture(String replyTopic)
    {
        super();
        this.replyTopic = replyTopic;
    }

    /**
     * Attempts to settle this request. Exactly one of reply-arrival, the framework deadline and
     * {@code cancelRequest} wins this CAS; the winner owns the cleanup (unsubscribe the reply
     * topic, remove the pending entry) and the completion of this future. The winner also cancels
     * the armed deadline timer (if any), so a settled request never fires a stale deadline.
     *
     * @return {@code true} for the settle winner; {@code false} when the request was already
     *         settled (the caller must no-op)
     */
    public boolean trySettle()
    {
        if (settled.compareAndSet(false, true))
        {
            ScheduledFuture<?> task = deadlineTask;
            if (task != null)
            {
                task.cancel(false);
            }
            return true;
        }
        return false;
    }

    /**
     * Whether this request has been settled (by reply-arrival, deadline or cancel).
     *
     * @return {@code true} once {@link #trySettle()} has been won
     */
    public boolean isSettled()
    {
        return settled.get();
    }

    /**
     * Attaches the framework-owned deadline timer for this request so the settle winner can cancel
     * it. If the request was already settled by the time the timer is attached (a reply can beat
     * the scheduling call), the timer is canceled immediately.
     *
     * @param task the scheduled deadline task
     */
    public void setDeadlineTask(ScheduledFuture<?> task)
    {
        this.deadlineTask = task;
        if (task != null && settled.get())
        {
            task.cancel(false);
        }
    }
}
