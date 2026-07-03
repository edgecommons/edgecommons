/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.mbreissi.ggcommons.config.provider;

import com.mbreissi.ggcommons.config.ConfigManager;
import com.google.gson.Gson;
import com.google.gson.JsonObject;

public abstract sealed class ConfigProvider
        permits FileConfigProvider, ConfigMapConfigProvider, EnvironmentConfigProvider,
                GreengrassConfigProvider, ShadowConfigProvider, ConfigComponentProvider {

   ConfigProvider(ConfigManager configManager)
   {
       this.parentConfigManager=configManager  ;
   }

    /**
     * Back-fills the parent {@link ConfigManager} after bootstrap. Providers are constructed by
     * {@code ConfigManagerFactory} <b>before</b> the {@code ConfigManager} exists (it is built
     * from the config the provider loads), so the constructor receives {@code null}; the
     * {@code ConfigManager} constructor calls this to attach itself as the hot-reload/push
     * target ({@code applyConfig}). Library-internal wiring — public only because
     * {@code ConfigManager} lives in the parent package.
     *
     * @param configManager the freshly constructed parent config manager (non-null)
     */
    public final void attachConfigManager(ConfigManager configManager)
    {
        this.parentConfigManager = java.util.Objects.requireNonNull(configManager,
                "configManager must not be null");
    }

   protected Gson gson=new Gson();
    protected ConfigManager parentConfigManager;
    public abstract JsonObject loadConfiguration();
    public abstract String getConfigSource();

    /** Releases any resources held by this provider (e.g. file watchers). Default no-op. */
    public void close() {}


}
