package com.mbreissi.edgecommons.logging;

/** Snapshot of the log bus publisher counters. */
public final class LogStats {
    private final long enqueuedRecords;
    private final long publishedRecords;
    private final long droppedRecords;
    private final long filteredRecords;
    private final long redactedRecords;
    private final long truncatedRecords;
    private final long publishFailures;
    private final int queuedRecords;

    public LogStats(long enqueuedRecords, long publishedRecords, long droppedRecords,
                    long filteredRecords, long redactedRecords, long truncatedRecords,
                    long publishFailures, int queuedRecords) {
        this.enqueuedRecords = enqueuedRecords;
        this.publishedRecords = publishedRecords;
        this.droppedRecords = droppedRecords;
        this.filteredRecords = filteredRecords;
        this.redactedRecords = redactedRecords;
        this.truncatedRecords = truncatedRecords;
        this.publishFailures = publishFailures;
        this.queuedRecords = queuedRecords;
    }

    public long getEnqueuedRecords() { return enqueuedRecords; }
    public long getPublishedRecords() { return publishedRecords; }
    public long getDroppedRecords() { return droppedRecords; }
    public long getFilteredRecords() { return filteredRecords; }
    public long getRedactedRecords() { return redactedRecords; }
    public long getTruncatedRecords() { return truncatedRecords; }
    public long getPublishFailures() { return publishFailures; }
    public int getQueuedRecords() { return queuedRecords; }
}
