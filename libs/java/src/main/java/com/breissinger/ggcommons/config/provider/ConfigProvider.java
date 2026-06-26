/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.breissinger.ggcommons.config.provider;

import com.breissinger.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonObject;

public abstract sealed class ConfigProvider
        permits FileConfigProvider, ConfigMapConfigProvider, EnvironmentConfigProvider,
                GreengrassConfigProvider, ShadowConfigProvider, ConfigComponentProvider {

   ConfigProvider(ConfigManager configManager)
   {
       this.parentConfigManager=configManager  ;
   }

   protected Gson gson=new Gson();
    protected ConfigManager parentConfigManager;
    public abstract JsonObject loadConfiguration();
    public abstract String getConfigSource();

    /** Releases any resources held by this provider (e.g. file watchers). Default no-op. */
    public void close() {}


}
