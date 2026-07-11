package com.mbreissi.edgecommons;

import com.mbreissi.edgecommons.commands.CommandInbox;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.mbreissi.edgecommons.config.ConfigurationCandidateValidator;
import org.apache.commons.cli.Options;

import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.function.Consumer;
import java.util.regex.Pattern;

/**
 * Builder for creating EdgeCommons instances with fluent API.
 */
public class EdgeCommonsBuilder {
    private static final Pattern VALIDATOR_NAME =
            Pattern.compile("^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$");
    private String componentName;
    private String[] args;
    private Options appOptions;
    private boolean receiveOwnMessages = true;
    private boolean initialReady = true;
    private Duration configValidationTimeout = ConfigManager.DEFAULT_CANDIDATE_VALIDATION_TIMEOUT;
    private final Map<String, ConfigurationCandidateValidator> configurationValidators =
            new LinkedHashMap<>();
    private final List<Consumer<CommandInbox>> commandConfigurers = new ArrayList<>();
    
    private EdgeCommonsBuilder() {}
    
    public static EdgeCommonsBuilder create(String componentName) {
        EdgeCommonsBuilder builder = new EdgeCommonsBuilder();
        builder.componentName = componentName;
        return builder;
    }
    
    public EdgeCommonsBuilder withArgs(String[] args) {
        this.args = args;
        return this;
    }
    
    public EdgeCommonsBuilder withAppOptions(Options appOptions) {
        this.appOptions = appOptions;
        return this;
    }
    
    public EdgeCommonsBuilder receiveOwnMessages(boolean receiveOwnMessages) {
        this.receiveOwnMessages = receiveOwnMessages;
        return this;
    }

    /** Sets the application readiness gate before any runtime initialization can become observable. */
    public EdgeCommonsBuilder initialReady(boolean ready) {
        this.initialReady = ready;
        return this;
    }

    /** Registers one ordered, side-effect-free pre-commit configuration validator. */
    public EdgeCommonsBuilder withConfigurationValidator(
            String name, ConfigurationCandidateValidator validator) {
        if (name == null || !VALIDATOR_NAME.matcher(name).matches()) {
            throw new IllegalArgumentException("configuration validator name must match "
                    + "^[A-Za-z0-9][A-Za-z0-9_.-]{0,63}$");
        }
        Objects.requireNonNull(validator, "configuration validator must not be null");
        if (configurationValidators.putIfAbsent(name, validator) != null) {
            throw new IllegalArgumentException("configuration validator '" + name
                    + "' is already registered");
        }
        return this;
    }

    /** Sets the overall deadline shared by one candidate generation's validators. */
    public EdgeCommonsBuilder withConfigValidationTimeout(Duration timeout) {
        Objects.requireNonNull(timeout, "configuration validation timeout must not be null");
        if (timeout.isZero() || timeout.isNegative()
                || timeout.compareTo(ConfigManager.MAX_CANDIDATE_VALIDATION_TIMEOUT) > 0) {
            throw new IllegalArgumentException("configuration validation timeout must be positive and at most "
                    + ConfigManager.MAX_CANDIDATE_VALIDATION_TIMEOUT.toSeconds() + " seconds");
        }
        this.configValidationTimeout = timeout;
        return this;
    }

    /** Installs application command handlers before the inbox subscription can become ACTIVE. */
    public EdgeCommonsBuilder configureCommands(Consumer<CommandInbox> configurer) {
        commandConfigurers.add(Objects.requireNonNull(configurer,
                "command configurer must not be null"));
        return this;
    }
    
    public EdgeCommons build() {
        if (componentName == null) {
            throw new IllegalStateException("Component name is required");
        }
        if (args == null) {
            args = new String[0];
        }
        
        EdgeCommons edgeCommons = new EdgeCommons();
        edgeCommons.init(componentName, args, appOptions, receiveOwnMessages, initialReady,
                new LinkedHashMap<>(configurationValidators), configValidationTimeout,
                List.copyOf(commandConfigurers));
        return edgeCommons;
    }
}
