/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.messaging;

import java.util.concurrent.CompletableFuture;

public class ReplyFuture extends CompletableFuture<Message>
{
    public String replyTopic;

    public ReplyFuture(String replyTopic)
    {
        super();
        this.replyTopic = replyTopic;
    }
}
