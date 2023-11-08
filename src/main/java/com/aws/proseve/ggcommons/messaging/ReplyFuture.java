package com.aws.proseve.ggcommons.messaging;

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
