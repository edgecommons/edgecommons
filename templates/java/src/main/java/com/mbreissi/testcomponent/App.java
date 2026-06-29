package <<PACKAGE>>;

import com.mbreissi.ggcommons.GGCommons;
import com.mbreissi.ggcommons.config.ConfigManager;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import static com.mbreissi.ggcommons.utils.Utils.sleep;

public class <<COMPONENTNAME>>
{
    private static final Logger LOGGER = LogManager.getLogger(<<COMPONENTNAME>>.class);

    GGCommons ggCommons;
    ConfigManager configManager;
 
    public static void main(String[] args) {
        new <<COMPONENTNAME>>(args);
    }

    public <<COMPONENTNAME>>(String[] args)
    {
        ggCommons = new GGCommons("<<COMPONENTFULLNAME>>", args);
        configManager = ggCommons.getConfigManager();
        while (true)
        {
            LOGGER.info("Running...");
            sleep(10000);
        }
    }
}