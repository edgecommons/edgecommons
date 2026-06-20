package <<PACKAGE>>;

import com.aws.proserve.ggcommons.GGCommons;
import com.aws.proserve.ggcommons.config.ConfigManager;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import static com.aws.proserve.ggcommons.utils.Utils.sleep;

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