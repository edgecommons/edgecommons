package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.github.cliftonlabs.json_simple.JsonException;
import com.github.cliftonlabs.json_simple.JsonObject;
import com.github.cliftonlabs.json_simple.Jsoner;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Paths;

class FileConfigProvider extends ConfigProvider
{
    private static final Logger LOGGER = LogManager.getLogger(FileConfigProvider.class);

    String configFilePath;

    FileConfigProvider(ConfigManager configManager, String configFilePath)
    {
        super(configManager);
        this.configFilePath = configFilePath;
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
    public String getConfigSource()
    {
        return String.format("Config File (path: %s)", configFilePath);
    }

    private String getFileAsString(File file) throws IOException
    {
        byte[] bytes = java.nio.file.Files.readAllBytes(Paths.get(file.getAbsolutePath()));
        return new String(bytes, StandardCharsets.UTF_8);
    }
}
