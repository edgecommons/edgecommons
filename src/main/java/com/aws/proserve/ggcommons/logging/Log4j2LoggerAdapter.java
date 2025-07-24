/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.logging;

import org.apache.logging.log4j.LogManager;

/**
 * Log4j2 implementation of the logger adapter.
 */
public class Log4j2LoggerAdapter implements LoggerAdapter {
    
    @Override
    public Logger getLogger(String name) {
        return new Log4j2Logger(LogManager.getLogger(name));
    }
    
    private static class Log4j2Logger implements Logger {
        private final org.apache.logging.log4j.Logger logger;
        
        Log4j2Logger(org.apache.logging.log4j.Logger logger) {
            this.logger = logger;
        }
        
        @Override
        public void trace(String message) {
            logger.trace(message);
        }
        
        @Override
        public void trace(String message, Object... args) {
            logger.trace(message, args);
        }
        
        @Override
        public void trace(String message, Throwable throwable) {
            logger.trace(message, throwable);
        }
        
        @Override
        public void debug(String message) {
            logger.debug(message);
        }
        
        @Override
        public void debug(String message, Object... args) {
            logger.debug(message, args);
        }
        
        @Override
        public void debug(String message, Throwable throwable) {
            logger.debug(message, throwable);
        }
        
        @Override
        public void info(String message) {
            logger.info(message);
        }
        
        @Override
        public void info(String message, Object... args) {
            logger.info(message, args);
        }
        
        @Override
        public void info(String message, Throwable throwable) {
            logger.info(message, throwable);
        }
        
        @Override
        public void warn(String message) {
            logger.warn(message);
        }
        
        @Override
        public void warn(String message, Object... args) {
            logger.warn(message, args);
        }
        
        @Override
        public void warn(String message, Throwable throwable) {
            logger.warn(message, throwable);
        }
        
        @Override
        public void error(String message) {
            logger.error(message);
        }
        
        @Override
        public void error(String message, Object... args) {
            logger.error(message, args);
        }
        
        @Override
        public void error(String message, Throwable throwable) {
            logger.error(message, throwable);
        }
        
        @Override
        public boolean isTraceEnabled() {
            return logger.isTraceEnabled();
        }
        
        @Override
        public boolean isDebugEnabled() {
            return logger.isDebugEnabled();
        }
        
        @Override
        public boolean isInfoEnabled() {
            return logger.isInfoEnabled();
        }
        
        @Override
        public boolean isWarnEnabled() {
            return logger.isWarnEnabled();
        }
        
        @Override
        public boolean isErrorEnabled() {
            return logger.isErrorEnabled();
        }
    }
}