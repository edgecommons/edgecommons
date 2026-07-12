"""<<COMPONENTNAME>> entry point — a southbound protocol-adapter on edgecommons.

Builds the framework, then spawns one worker thread per ``component.instances[]`` entry (each device
connects/retries independently). The library owns SIGTERM/SIGINT -> graceful shutdown.
"""
import argparse
import logging
import sys
import threading

from edgecommons import EdgeCommonsBuilder

from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(description="<<COMPONENTNAME>> southbound adapter")
    gg = (
        EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .initial_ready(False)
        .build()
    )
    config_manager = gg.get_config_manager()

    devices = []

    def worker(instance_id):
        try:
            device = <<COMPONENTNAME>>(gg, instance_id)
            devices.append(device)
            gg.set_ready(True)
            device.run()
        except Exception:  # noqa: BLE001
            logger.exception("[%s] failed", instance_id)

    for instance_id in config_manager.get_instance_ids():
        threading.Thread(target=worker, args=(instance_id,), name=f"adapter-{instance_id}", daemon=True).start()

    try:
        threading.Event().wait()
    finally:
        for d in devices:
            try:
                d.stop()
            except Exception:  # noqa: BLE001
                pass
        gg.shutdown()


if __name__ == "__main__":
    main()
