package com.mbreissi.edgecommons.messaging.proto;

public enum MessageBodyCase {
    SOUTHBOUND_SIGNAL_UPDATE,
    STATE_UPDATE,
    CONFIG_UPDATE,
    METRIC_UPDATE,
    EVENT,
    COMMAND,
    STRUCTURED,
    OPAQUE,
    BODY_NOT_SET
}
