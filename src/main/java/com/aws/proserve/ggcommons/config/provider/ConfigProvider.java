/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config.provider;

import com.aws.proserve.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonObject;

public abstract class ConfigProvider {

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
