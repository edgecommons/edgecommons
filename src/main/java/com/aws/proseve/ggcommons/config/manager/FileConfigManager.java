package com.aws.proseve.ggcommons.config.manager;

import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Paths;

class FileConfigManager extends ConfigManager
{
    String configFilePath;

    FileConfigManager(String componentName, String configFilePath)
    {
        super(componentName);
        this.configFilePath = configFilePath;
        init();
    }

    @Override
    protected JsonObject loadConfiguration()
    {
        LOGGER.debug("Loading configuration from file '{}'", configFilePath);
        JsonObject retVal = null;
        try
        {
            File file = new File(configFilePath);
            String configurationFileContents = getFileAsString(file);
            retVal = (JsonObject) Jsoner.deserialize(configurationFileContents);
        }
        catch (JsonException | IOException e)
        {
            LOGGER.fatal("Error reading configuration file '{}': {}", configFilePath, e.toString());
            System.exit(1);
        }

        return retVal;
    }

    @Override
    protected String getConfigSource()
    {
        return String.format("Config File (path: %s)", configFilePath);
    }

    private String getFileAsString(File file) throws IOException
    {
        byte[] bytes = java.nio.file.Files.readAllBytes(Paths.get(file.getAbsolutePath()));
        return new String(bytes, StandardCharsets.UTF_8);
    }
}
