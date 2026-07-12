"""<<COMPONENTNAME>> entry point — a southbound protocol-adapter on edgecommons.

Builds the framework, then spawns one worker thread per ``component.instances[]`` entry (each device
connects/retries independently). The library owns SIGTERM/SIGINT -> graceful shutdown.
"""
import argparse
import logging
import sys
import threading

from edgecommons import EdgeCommonsBuilder

from app.<<COMPONENTNAME>> import <<COMPONENTNAME>>, link_statuses

logger = logging.getLogger("main")


def main():
    arg_parser = argparse.ArgumentParser(description="<<COMPONENTNAME>> southbound adapter")
    gg = (
        EdgeCommonsBuilder.create("<<COMPONENTFULLNAME>>")
        .with_args(sys.argv[1:])
        .with_app_options(arg_parser)
        .build()
    )
    config_manager = gg.get_config_manager()
    gg.set_ready(False)

    devices = []

    # A LinkStatus for EVERY configured device, built before any worker runs — so a device that is
    # configured but down is reported (connected=false / CONNECTING) from the very first keepalive.
    # A configured device that is down must never be indistinguishable from one that was never
    # configured at all.
    links = link_statuses(config_manager)

    # ONE provider, TWO surfaces: the library pushes this sample into the `state` keepalive's
    # instances[] on every tick AND returns it from the built-in `status` verb when a console asks —
    # so whoever watches and whoever asks can never get different answers. One entry per device is
    # the point of the adapter archetype: the fleet sees which of THIS component's devices are
    # reachable, without minting a UNS instance per connection. Keep it cheap: it is sampled on the
    # keepalive interval.
    gg.set_instance_connectivity_provider(lambda: [link.connectivity() for link in links.values()])

    def worker(instance_id):
        try:
            device = <<COMPONENTNAME>>(gg, instance_id, links[instance_id])
            devices.append(device)
            gg.set_ready(True)
            device.run()
        except Exception:  # noqa: BLE001
            logger.exception("[%s] failed", instance_id)

    for instance_id in links:
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
