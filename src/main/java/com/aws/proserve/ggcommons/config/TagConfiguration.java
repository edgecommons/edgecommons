/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */
package com.aws.proserve.ggcommons.config;

import com.google.gson.Gson;
import com.google.gson.JsonObject;

import java.util.Set;

public class TagConfiguration
{
    JsonObject tags = new JsonObject();

    public TagConfiguration(JsonObject jsonConfig)
    {
        if (jsonConfig != null)
        {
            tags = jsonConfig;
        }
    }

    public JsonObject toDict()
    {
        return tags;
    }

    @Override
    public String toString()
    {
        return new Gson().toJson(toDict());
    }

    public Set<String> getKeys() {
        return tags.keySet();
    }

    public String getKeyValue(String key) {
        return tags.get(key).getAsString();
    }
}
