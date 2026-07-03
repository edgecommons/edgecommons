package <<PACKAGE>>;

import com.mbreissi.ggcommons.GGCommons;
import com.mbreissi.ggcommons.GGCommonsBuilder;
import com.mbreissi.ggcommons.config.ConfigManager;
import com.mbreissi.ggcommons.messaging.Message;
import com.mbreissi.ggcommons.messaging.MessageBuilder;
import com.mbreissi.ggcommons.messaging.MessagingClient;
import com.mbreissi.ggcommons.uns.UnsClass;
import com.google.gson.JsonObject;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import static com.mbreissi.ggcommons.utils.Utils.sleep;

/**
 * Minimal GGCommons component scaffold.
 *
 * <p>The library gives you config, messaging, metrics, logging and lifecycle; identity is
 * <b>config-driven</b>: the top-level {@code hierarchy} + {@code identity} config blocks resolve
 * to this component's UNS identity (the last hierarchy level is always the resolved thing name).
 * Every envelope built with {@code .withConfig(...)} carries that identity automatically, and
 * every topic is minted through {@code gg.getUns()} — never hand-written.
 *
 * <p>The {@code state} heartbeat keepalive is <b>automatic</b> (library-owned, on / 5 s / local
 * transport by default) on {@code ecv1/{device}/{component}/main/state} — no heartbeat code here.
 */
public class <<COMPONENTNAME>>
{
    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    final GGCommons ggCommons;
    final ConfigManager configManager;
    final MessagingClient messaging;

    public static void main(String[] args) {
        new <<COMPONENTNAME>>(args);
    }

    public <<COMPONENTNAME>>(String[] args)
    {
        ggCommons = GGCommonsBuilder.create("<<COMPONENTFULLNAME>>").withArgs(args).build();
        configManager = ggCommons.getConfigManager();
        messaging = ggCommons.getMessaging();

        // The resolved UNS identity path (e.g. "site1/my-gw") and a topic minted from it.
        // APP is the free application class: ecv1/{device}/{component}/main/app/{channel...}.
        // (For per-instance topics/messages use gg.instance("<id>").uns() / .newMessage(...).)
        String statusTopic = ggCommons.getUns().topic(UnsClass.APP, "status");
        LOGGER.info("UNS identity path: {} - publishing status to {}",
                ggCommons.getUns().identity().getPath(), statusTopic);

        long seq = 0;
        while (true)
        {
            JsonObject body = new JsonObject();
            body.addProperty("seq", ++seq);
            body.addProperty("message", "Hello from <<COMPONENTNAME>>");

            // .withConfig(...) stamps the envelope's `identity` block (instance "main")
            // automatically; building without it is legal only for identity-free bootstrap/raw
            // messages.
            Message msg = MessageBuilder.create("StatusUpdate", "1.0")
                    .withPayload(body)
                    .withConfig(configManager)
                    .build();
            messaging.publish(statusTopic, msg);

            LOGGER.info("Running... (published status seq={})", seq);
            sleep(10000);
        }
    }
}
