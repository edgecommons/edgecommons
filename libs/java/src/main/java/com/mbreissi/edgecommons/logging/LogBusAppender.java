package com.mbreissi.edgecommons.logging;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.core.Appender;
import org.apache.logging.log4j.core.Filter;
import org.apache.logging.log4j.core.LogEvent;
import org.apache.logging.log4j.core.LoggerContext;
import org.apache.logging.log4j.core.appender.AbstractAppender;
import org.apache.logging.log4j.core.config.Configuration;
import org.apache.logging.log4j.core.config.LoggerConfig;
import org.apache.logging.log4j.core.config.Property;
import org.apache.logging.log4j.core.impl.Log4jContextFactory;
import org.apache.logging.log4j.spi.LoggerContextFactory;

import java.util.LinkedHashSet;
import java.util.Set;

/** Log4j2 appender that forwards log events to the EdgeCommons log bus service. */
public final class LogBusAppender extends AbstractAppender {
    public static final String APPENDER_NAME = "EdgeCommonsLogBus";

    private LogBusAppender(String name, Filter filter) {
        super(name, filter, null, true, Property.EMPTY_ARRAY);
    }

    @Override
    public void append(LogEvent event) {
        LogBusCapture.capture(event.toImmutable());
    }

    /** Attaches the appender to the current root and explicit logger configs. */
    public static void install(LoggerContext context, boolean enabled) {
        for (LoggerContext loggerContext : loggerContexts(context)) {
            if (enabled) {
                configure(loggerContext.getConfiguration(), true);
            } else {
                uninstallFromContext(loggerContext);
            }
            loggerContext.updateLoggers();
        }
    }

    /** Attaches or removes the appender from every active Log4j2 context visible in this process. */
    public static void installAll(boolean enabled) {
        install(null, enabled);
    }

    /**
     * Adds this appender to a Log4j configuration before or after the configuration is started.
     *
     * <p>Pre-wiring it into a just-built configuration is more reliable for already-created static
     * loggers than mutating only the live configuration after {@code LoggerContext.start(...)}.
     */
    public static synchronized void configure(Configuration config, boolean enabled) {
        if (config == null) {
            return;
        }
        if (!enabled) {
            detach(config.getRootLogger());
            for (LoggerConfig loggerConfig : config.getLoggers().values()) {
                detach(loggerConfig);
            }
            Appender appender = config.getAppender(APPENDER_NAME);
            if (appender != null) {
                appender.stop();
            }
            return;
        }
        Appender appender = config.getAppender(APPENDER_NAME);
        if (appender == null) {
            appender = new LogBusAppender(APPENDER_NAME, null);
            appender.start();
            config.addAppender(appender);
        } else if (!appender.isStarted()) {
            appender.start();
        }
        attach(config.getRootLogger(), appender);
        for (LoggerConfig loggerConfig : config.getLoggers().values()) {
            if (!loggerConfig.isAdditive()) {
                attach(loggerConfig, appender);
            }
        }
    }

    static void uninstall(LoggerContext context) {
        if (context == null) {
            return;
        }
        uninstallFromContext(context);
        context.updateLoggers();
    }

    static void uninstallAll() {
        installAll(false);
    }

    private static void uninstallFromContext(LoggerContext context) {
        Configuration config = context.getConfiguration();
        detach(config.getRootLogger());
        for (LoggerConfig loggerConfig : config.getLoggers().values()) {
            detach(loggerConfig);
        }
        Appender appender = config.getAppender(APPENDER_NAME);
        if (appender != null) {
            appender.stop();
        }
    }

    private static void attach(LoggerConfig loggerConfig, Appender appender) {
        if (loggerConfig.getAppenders().containsKey(APPENDER_NAME)) {
            return;
        }
        loggerConfig.addAppender(appender, null, null);
    }

    private static void detach(LoggerConfig loggerConfig) {
        if (loggerConfig.getAppenders().containsKey(APPENDER_NAME)) {
            loggerConfig.removeAppender(APPENDER_NAME);
        }
    }

    private static Set<LoggerContext> loggerContexts(LoggerContext preferred) {
        Set<LoggerContext> contexts = new LinkedHashSet<>();
        addContext(contexts, preferred);
        try {
            addContext(contexts, LogManager.getContext(false));
        } catch (RuntimeException ignored) {
            // Best effort: context discovery must never break application logging.
        }
        try {
            addContext(contexts, LogManager.getContext(true));
        } catch (RuntimeException ignored) {
            // Best effort: context discovery must never break application logging.
        }
        try {
            LoggerContextFactory factory = LogManager.getFactory();
            if (factory instanceof Log4jContextFactory log4jFactory) {
                for (LoggerContext context : log4jFactory.getSelector().getLoggerContexts()) {
                    addContext(contexts, context);
                }
            }
        } catch (RuntimeException ignored) {
            // Some hosts use custom factories/selectors; native capture stays opportunistic there.
        }
        return contexts;
    }

    private static void addContext(Set<LoggerContext> contexts,
                                   org.apache.logging.log4j.spi.LoggerContext context) {
        if (context instanceof LoggerContext loggerContext) {
            contexts.add(loggerContext);
        }
    }
}
