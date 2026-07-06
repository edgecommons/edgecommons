/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.logging;

import java.util.logging.Level;

/**
 * Java Util Logging (JUL) implementation of the logger adapter.
 * Used as fallback when no other logging framework is available.
 */
public class JulLoggerAdapter implements LoggerAdapter {
    
    @Override
    public Logger getLogger(String name) {
        return new JulLogger(java.util.logging.Logger.getLogger(name));
    }
    
    private static class JulLogger implements Logger {
        private final java.util.logging.Logger logger;
        
        JulLogger(java.util.logging.Logger logger) {
            this.logger = logger;
        }
        
        @Override
        public void trace(String message) {
            logger.log(Level.FINEST, message);
        }
        
        @Override
        public void trace(String message, Object... args) {
            logger.log(Level.FINEST, String.format(message.replace("{}", "%s"), args));
        }
        
        @Override
        public void trace(String message, Throwable throwable) {
            logger.log(Level.FINEST, message, throwable);
        }
        
        @Override
        public void debug(String message) {
            logger.log(Level.FINE, message);
        }
        
        @Override
        public void debug(String message, Object... args) {
            logger.log(Level.FINE, String.format(message.replace("{}", "%s"), args));
        }
        
        @Override
        public void debug(String message, Throwable throwable) {
            logger.log(Level.FINE, message, throwable);
        }
        
        @Override
        public void info(String message) {
            logger.log(Level.INFO, message);
        }
        
        @Override
        public void info(String message, Object... args) {
            logger.log(Level.INFO, String.format(message.replace("{}", "%s"), args));
        }
        
        @Override
        public void info(String message, Throwable throwable) {
            logger.log(Level.INFO, message, throwable);
        }
        
        @Override
        public void warn(String message) {
            logger.log(Level.WARNING, message);
        }
        
        @Override
        public void warn(String message, Object... args) {
            logger.log(Level.WARNING, String.format(message.replace("{}", "%s"), args));
        }
        
        @Override
        public void warn(String message, Throwable throwable) {
            logger.log(Level.WARNING, message, throwable);
        }
        
        @Override
        public void error(String message) {
            logger.log(Level.SEVERE, message);
        }
        
        @Override
        public void error(String message, Object... args) {
            logger.log(Level.SEVERE, String.format(message.replace("{}", "%s"), args));
        }
        
        @Override
        public void error(String message, Throwable throwable) {
            logger.log(Level.SEVERE, message, throwable);
        }
        
        @Override
        public boolean isTraceEnabled() {
            return logger.isLoggable(Level.FINEST);
        }
        
        @Override
        public boolean isDebugEnabled() {
            return logger.isLoggable(Level.FINE);
        }
        
        @Override
        public boolean isInfoEnabled() {
            return logger.isLoggable(Level.INFO);
        }
        
        @Override
        public boolean isWarnEnabled() {
            return logger.isLoggable(Level.WARNING);
        }
        
        @Override
        public boolean isErrorEnabled() {
            return logger.isLoggable(Level.SEVERE);
        }
    }
}