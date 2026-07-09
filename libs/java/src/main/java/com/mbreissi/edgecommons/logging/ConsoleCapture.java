package com.mbreissi.edgecommons.logging;

import java.io.IOException;
import java.io.OutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

final class ConsoleCapture {
    private static final Pattern JAVA_LOG_LINE = Pattern.compile(
            "^\\d{4}-\\d{2}-\\d{2} \\d{2}:\\d{2}:\\d{2}(?:\\.\\d{3})? \\[(TRACE|DEBUG|INFO|WARN|ERROR|FATAL)\\s*]\\s+(.+?)\\s*\\(\\s*\\d+\\)\\s*(?:\\[([^]]+)\\])?\\s*:\\s*(.*)$");

    private static PrintStream originalOut;
    private static PrintStream originalErr;
    private static boolean installed;

    private ConsoleCapture() {}

    static synchronized void configure(LogService service, boolean enabled) {
        if (enabled && !installed) {
            originalOut = System.out;
            originalErr = System.err;
            System.setOut(new PrintStream(new CapturingOutputStream(originalOut,
                    "console.stdout", LogLevel.INFO), true, StandardCharsets.UTF_8));
            System.setErr(new PrintStream(new CapturingOutputStream(originalErr,
                    "console.stderr", LogLevel.ERROR), true, StandardCharsets.UTF_8));
            installed = true;
        } else if (!enabled && installed) {
            System.setOut(originalOut);
            System.setErr(originalErr);
            installed = false;
        }
    }

    private static final class CapturingOutputStream extends OutputStream {
        private final PrintStream delegate;
        private final String logger;
        private final LogLevel level;
        private final StringBuilder line = new StringBuilder();

        private CapturingOutputStream(PrintStream delegate, String logger, LogLevel level) {
            this.delegate = delegate;
            this.logger = logger;
            this.level = level;
        }

        @Override
        public synchronized void write(int b) throws IOException {
            delegate.write(b);
            if (b == '\n') {
                emit();
            } else if (b != '\r') {
                line.append((char) b);
            }
        }

        @Override
        public synchronized void flush() {
            delegate.flush();
            emit();
        }

        private void emit() {
            if (line.isEmpty()) {
                return;
            }
            String text = line.toString();
            LogRecord parsed = parseJavaLogLine(text);
            if (parsed != null) {
                LogBusCapture.captureConsole(parsed);
            } else {
                LogBusCapture.captureConsole(logger, level, text);
            }
            line.setLength(0);
        }

        private LogRecord parseJavaLogLine(String text) {
            Matcher matcher = JAVA_LOG_LINE.matcher(text);
            if (!matcher.matches()) {
                return null;
            }
            LogRecord.Builder builder = LogRecord.builder()
                    .withLevel(matcher.group(1))
                    .withLogger(matcher.group(2).trim())
                    .withMessage(matcher.group(4));
            if (matcher.group(3) != null && !matcher.group(3).isBlank()) {
                builder.withThread(matcher.group(3).trim());
            }
            return builder.build();
        }
    }
}
