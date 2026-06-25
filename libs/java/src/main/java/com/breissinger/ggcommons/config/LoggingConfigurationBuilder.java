package com.breissinger.ggcommons.config;

import com.google.gson.JsonObject;
import org.apache.logging.log4j.Level;
import java.util.HashMap;
import java.util.Map;

/**
 * Builder for creating LoggingConfiguration instances programmatically.
 */
public class LoggingConfigurationBuilder {
    private String level = "INFO";
    private String format = "%d{yyyy-MM-dd HH:mm:ss} [%-5p] %-25.25c{1}(%4L): %m%n";
    private boolean fileLoggingEnabled = false;
    private String logFilePath = null;
    private Map<String, String> loggerLevels = new HashMap<>();
    private boolean globalControl = false;
    
    private LoggingConfigurationBuilder() {}
    
    public static LoggingConfigurationBuilder create() {
        return new LoggingConfigurationBuilder();
    }
    
    public LoggingConfigurationBuilder withLevel(Level level) {
        this.level = level.toString();
        return this;
    }
    
    public LoggingConfigurationBuilder withLevel(String level) {
        this.level = level;
        return this;
    }
    
    public LoggingConfigurationBuilder withFormat(String format) {
        this.format = format;
        return this;
    }
    
    public LoggingConfigurationBuilder enableFileLogging(String filePath) {
        this.fileLoggingEnabled = true;
        this.logFilePath = filePath;
        return this;
    }
    
    public LoggingConfigurationBuilder disableFileLogging() {
        this.fileLoggingEnabled = false;
        this.logFilePath = null;
        return this;
    }
    
    public LoggingConfigurationBuilder addLogger(String loggerName, Level level) {
        this.loggerLevels.put(loggerName, level.toString());
        return this;
    }
    
    public LoggingConfigurationBuilder addLogger(String loggerName, String level) {
        this.loggerLevels.put(loggerName, level);
        return this;
    }
    
    public LoggingConfigurationBuilder enableGlobalControl() {
        this.globalControl = true;
        return this;
    }
    
    public LoggingConfiguration build() {
        JsonObject config = new JsonObject();
        config.addProperty("level", level);
        config.addProperty("format", format);
        
        if (fileLoggingEnabled) {
            JsonObject fileConfig = new JsonObject();
            fileConfig.addProperty("enabled", true);
            if (logFilePath != null) {
                fileConfig.addProperty("filePath", logFilePath);
            }
            config.add("fileLogging", fileConfig);
        }
        
        if (!loggerLevels.isEmpty()) {
            JsonObject loggersConfig = new JsonObject();
            for (Map.Entry<String, String> entry : loggerLevels.entrySet()) {
                loggersConfig.addProperty(entry.getKey(), entry.getValue());
            }
            config.add("loggers", loggersConfig);
        }
        
        if (globalControl) {
            config.addProperty("globalControl", true);
        }
        
        return new LoggingConfiguration(config);
    }
}