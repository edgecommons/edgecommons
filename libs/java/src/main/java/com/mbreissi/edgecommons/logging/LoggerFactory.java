/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.edgecommons.logging;

/**
 * Factory for creating loggers that can adapt to different logging frameworks.
 * This provides a compatibility layer between EdgeCommons and various logging implementations.
 */
public class LoggerFactory {
    
    private static LoggerAdapter adapter;
    
    static {
        // Auto-detect available logging framework
        if (isSlf4jAvailable()) {
            adapter = new Slf4jLoggerAdapter();
        } else if (isLog4j2Available()) {
            adapter = new Log4j2LoggerAdapter();
        } else {
            adapter = new JulLoggerAdapter();
        }
    }
    
    public static Logger getLogger(Class<?> clazz) {
        return adapter.getLogger(clazz.getName());
    }
    
    public static Logger getLogger(String name) {
        return adapter.getLogger(name);
    }
    
    private static boolean isLog4j2Available() {
        try {
            Class.forName("org.apache.logging.log4j.LogManager");
            return true;
        } catch (ClassNotFoundException e) {
            return false;
        }
    }
    
    private static boolean isSlf4jAvailable() {
        try {
            Class.forName("org.slf4j.LoggerFactory");
            return true;
        } catch (ClassNotFoundException e) {
            return false;
        }
    }
}