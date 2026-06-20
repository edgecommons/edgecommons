"""On-device verification (Python) of the sanitized default SHADOW name.

Run as a Greengrass component with `-c SHADOW` (no explicit name): the SHADOW config
manager defaults the shadow name to the component name and sanitizes it
(``com.ggcommons.PyShadowVerify`` -> ``com_ggcommons_PyShadowVerify``), then loads
config from that named shadow via ShadowManager IPC. ``get_config_source()`` reports
the resolved shadow name, and the loaded marker values (set in the cloud shadow under
the sanitized name) prove the default->sanitize->GetThingShadow path runs end-to-end.
"""
import json
import sys

from ggcommons import GGCommonsBuilder

COMPONENT = "com.ggcommons.PyShadowVerify"
RESULT = "/tmp/python_shadow_verify_result.json"


def main():
    out = {"lang": "python"}
    try:
        gg = (
            GGCommonsBuilder.create(COMPONENT)
            .with_args(sys.argv[1:])
            .receive_own_messages(False)
            .build()
        )
        cm = gg.get_config_manager()
        out["connected"] = True
        out["config_source"] = cm.get_config_source()  # includes the resolved shadow name
        out["config_loaded"] = {
            "publish_interval": (cm.get_global_config() or {}).get("publish_interval"),
            "site": (cm.get_full_config().get("tags") or {}).get("site"),
            "thing": cm.get_thing_name(),
        }
        with open(RESULT, "w") as f:
            json.dump(out, f)
        gg.shutdown()
    except Exception as e:  # noqa: BLE001 - harness records any failure
        out["connected"] = False
        out["error"] = str(e)
        with open(RESULT, "w") as f:
            json.dump(out, f)
        sys.exit(1)


if __name__ == "__main__":
    main()
