package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.github.cliftonlabs.json_simple.JsonObject;

public abstract class ConfigProvider {

   ConfigProvider(ConfigManager configManager)
   {
       this.parentConfigManager=configManager  ;
   }

    protected ConfigManager parentConfigManager;
    public abstract JsonObject loadConfiguration();
    public abstract String getConfigSource();


}
