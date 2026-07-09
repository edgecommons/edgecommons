package com.mbreissi.edgecommons.logging;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.core.LogEvent;

import java.io.PrintWriter;
import java.io.StringWriter;
import java.time.Instant;

final class LogBusCapture {
    private static volatile LogService service;

    private LogBusCapture() {}

    static void setService(LogService newService) {
        service = newService;
    }

    static void clearService(LogService oldService) {
        if (service == oldService) {
            service = null;
        }
    }

    static void capture(LogEvent event) {
        LogService current = service;
        if (current == null || LogService.isPublishingThread()) {
            return;
        }
        LogRecord.Builder builder = LogRecord.builder()
                .withTimestamp(Instant.ofEpochMilli(event.getTimeMillis()))
                .withLevel(LogLevel.fromLog4j(event.getLevel()))
                .withLogger(event.getLoggerName() == null ? "root" : event.getLoggerName())
                .withMessage(event.getMessage() == null ? "" : event.getMessage().getFormattedMessage())
                .withThread(event.getThreadName());
        if (event.getThrown() != null) {
            JsonObject error = new JsonObject();
            error.addProperty("type", event.getThrown().getClass().getName());
            error.addProperty("message", event.getThrown().getMessage());
            StringWriter writer = new StringWriter();
            event.getThrown().printStackTrace(new PrintWriter(writer));
            error.addProperty("stack", writer.toString());
            builder.withError(error);
        }
        current.captureNative(builder.build());
    }

    static void captureConsole(String logger, LogLevel level, String message) {
        LogService current = service;
        if (current == null || LogService.isPublishingThread() || message == null || message.isEmpty()) {
            return;
        }
        captureConsole(LogRecord.builder()
                .withTimestamp(Instant.now())
                .withLevel(level)
                .withLogger(logger)
                .withMessage(message)
                .withThread(Thread.currentThread().getName())
                .build());
    }

    static void captureConsole(LogRecord record) {
        LogService current = service;
        if (current == null || LogService.isPublishingThread() || record == null) {
            return;
        }
        current.captureConsole(record);
    }
}
