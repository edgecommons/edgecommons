package com.mbreissi.edgecommons.shadowverify;

import com.mbreissi.edgecommons.EdgeCommons;
import com.mbreissi.edgecommons.EdgeCommonsBuilder;
import com.mbreissi.edgecommons.config.ConfigManager;
import com.google.gson.JsonObject;

import java.nio.file.Files;
import java.nio.file.Paths;

/**
 * On-device verification (Java) of the sanitized default SHADOW name.
 *
 * Run as a Greengrass component with {@code -c SHADOW} (no explicit name): the SHADOW
 * config provider defaults the shadow name to the component name and sanitizes it
 * ({@code com.mbreissi.edgecommons.JavaShadowVerify} -> {@code com_mbreissi_edgecommons_JavaShadowVerify}),
 * then loads config from that named shadow via ShadowManager IPC. Loading the marker
 * values (set in the cloud shadow under the sanitized name) proves the
 * default->sanitize->GetThingShadow path runs end-to-end.
 */
public final class ShadowVerify {
    private static final String COMPONENT = "com.mbreissi.edgecommons.JavaShadowVerify";
    private static final String RESULT = "/tmp/java_shadow_verify_result.json";

    public static void main(String[] args) {
        try {
            EdgeCommons gg = EdgeCommonsBuilder.create(COMPONENT).withArgs(args).build();
            ConfigManager cm = gg.getConfigManager();

            JsonObject global = cm.getGlobalConfig();
            String publishInterval = (global != null && global.has("publish_interval"))
                    ? global.get("publish_interval").getAsString() : "null";
            String site = (cm.getTagConfig() != null) ? cm.getTagConfig().getKeyValue("site") : null;

            String json = String.format(
                    "{\"lang\":\"java\",\"connected\":true,\"config_loaded\":"
                            + "{\"publish_interval\":%s,\"site\":\"%s\",\"thing\":\"%s\"}}",
                    publishInterval, site, cm.getThingName());
            Files.write(Paths.get(RESULT), json.getBytes());

            Thread.sleep(20000); // stay RUNNING briefly, then exit
            System.exit(0);
        } catch (Exception e) {
            try {
                Files.write(Paths.get(RESULT),
                        ("{\"lang\":\"java\",\"connected\":false,\"error\":\""
                                + e.toString().replace("\"", "'") + "\"}").getBytes());
            } catch (Exception ignored) {
                // best effort
            }
            System.exit(1);
        }
    }
}
