# ggcommons-python-lib

`ggcommons` is an open source framework that makes it easier to build reusable, robust, and monitorable components for AWS IoT Greengrass. Some key capabilities it provides include:
1. Configuration management to simplify how components retrieve configuration settings
2. Messaging support with enhancements like request-response patterns and message structures
3. Monitoring at the individual component level for better visibility into system health and performance
4. Local debugging to streamline the development process
5. Reusable components that work well together due to following common patterns and interfaces

`ggcommons` aims to reduce barriers to creating industrial strength Greengrass components suitable for industrial IoT scenarios. Some attributes of industrial strength include flexibility in deployment models, high availability, and ability to handle real-time data and events from industrial equipment and processes.

## Getting started

## Local development and testing

Local testing is possible with the set up of a local MQTT server that acts as a local instance of Greengrass IPC. The MQTT messaging provider also allows for mocking request/response patterns between components to validate behavior. 

> :warning: If your component has a hard dependency on other components, ensure that these components are also running in "local" mode. 

### Quickstart

#### Set up local MQTT broker

1. Install [Docker Engine](https://docs.docker.com/engine/install/).
2. Install [MQTTX desktop (and optionally CLI) client](https://mqttx.app/downloads).
3. Run `docker run -d --name emqx -p 1883:1883 -p 8083:8083 -p 8084:8084 -p 8883:8883 -p 18083:18083 emqx/emqx:latest`
4. Open the MQTTX desktop app and connect to `localhost`. 

#### Set up your component source

1. Refer to the [Python component skeleton](https://gitlab.aws.dev/greengrass-commons/python-component-skeleton).
2. Your entrypoint and dependencies need to be configured to use `ggcommons`. The `ggcommons.init()` function defines the messaging and config provider that is needed. For local run/testing, the messaging provider is `MQTT` and the config provider is `FILE`. 

#### Run your component locally

1. Run your component's entry point with the `-m MQTT` flag. If you need to configure your component, pass the `-c FILE <YOUR JSON CONFIG FILE>` flag E.g.:
```bash
python3 main.py -m MQTT -c FILE "test_config.json"
```
2. Navigate to the MQTTX desktop app and subscribe to `heartbeat/+/+` and you should see your component's heartbeat messages.
3. If your component is configured to publish messages, subscribe to the relevant topics to view them. 
4. To send messages to your component, publish messages to the relevant topics using MQTTX. 

## Contributing
State if you are open to contributions and what your requirements are for accepting them.

For people who want to make changes to your project, it's helpful to have some documentation on how to get started. Perhaps there is a script that they should run or some environment variables that they need to set. Make these steps explicit. These instructions could also be useful to your future self.

You can also document commands to lint the code or run tests. These steps help to ensure high code quality and reduce the likelihood that the changes inadvertently break something. Having instructions for running tests is especially helpful if it requires external setup, such as starting a Selenium server for testing in a browser.

## Authors and acknowledgment
Show your appreciation to those who have contributed to the project.

## License
For open source projects, say how it is licensed.

## Project status
If you have run out of energy or time for your project, put a note at the top of the README saying that development has slowed down or stopped completely. Someone may choose to fork your project or volunteer to step in as a maintainer or owner, allowing your project to keep going. You can also make an explicit request for maintainers.
