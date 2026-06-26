/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.utils.FileWatcher;
import com.google.gson.JsonObject;
import com.google.gson.JsonSyntaxException;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Paths;

final class FileConfigProvider extends ConfigProvider implements FileWatcher.FileChangeHandler
{
    private static final Logger LOGGER = LogManager.getLogger(FileConfigProvider.class);

    String configFilePath;

    final FileWatcher configFileWatcher;

    FileConfigProvider(ConfigManager configManager, String configFilePath)
    {
        super(configManager);
        this.configFilePath = configFilePath;
        this.configFileWatcher = new FileWatcher(configFilePath, this);
        configFileWatcher.setDaemon(true);
        configFileWatcher.start();
    }

    @Override
    public void close()
    {
        configFileWatcher.stopThread();
    }

    @Override
    public JsonObject loadConfiguration()
    {
        LOGGER.debug("Loading configuration from file '{}'", configFilePath);
        JsonObject retVal = null;
        try
        {
            File file = new File(configFilePath);
            String configurationFileContents = getFileAsString(file);
            retVal = gson.fromJson(configurationFileContents, JsonObject.class);
        }
        catch (JsonSyntaxException | IOException e)
        {
            LOGGER.fatal("Error reading configuration file '{}': {}", configFilePath, e.toString());
            throw new RuntimeException("Error reading configuration file '" + configFilePath + "': " + e, e);
        }

        return retVal;
    }

    @Override
    public String getConfigSource()
    {
        return String.format("Config File (path: %s)", configFilePath);
    }

    private String getFileAsString(File file) throws IOException
    {
        byte[] bytes = java.nio.file.Files.readAllBytes(Paths.get(file.getAbsolutePath()));
        return new String(bytes, StandardCharsets.UTF_8);
    }

    @Override
    public void onChange()
    {
        JsonObject newConfig = loadConfiguration();
        LOGGER.info("configurationChanged: Applying new config: {}", newConfig);
        parentConfigManager.applyConfig(newConfig);
    }
}
