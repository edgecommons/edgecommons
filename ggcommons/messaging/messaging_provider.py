import abc
from abc import abstractmethod
from typing import Callable
from ggcommons.messaging.message import Message


class MessagingProvider(metaclass=abc.ABCMeta):

    def __init__(self):
        pass

    @abstractmethod
    def publish(self, topic: str, msg: Message):
        pass

    @abstractmethod
    def subscribe(self, topic: str, callback: Callable[[str, Message], None]):
        pass

    @abstractmethod
    def unsubscribe(self, topic: str):
        pass

    # Copied from open source Paho MQTT python client
    # (https://github.com/thejuan/paho-mqtt-python/blob/master/src/paho/mqtt/client.py)
    # Under the Eclipse Public License (https://github.com/thejuan/paho-mqtt-python/blob/master/LICENSE.txt)
    @staticmethod
    def topic_matches_sub(sub: str, topic: str) -> bool:
        """Check whether a topic matches a subscription.
        For example:
        foo/bar would match the subscription foo/# or +/bar
        non/matching would not match the subscription non/+/+
        """
        result = True
        multilevel_wildcard = False

        slen = len(sub)
        tlen = len(topic)

        if slen > 0 and tlen > 0:
            if (sub[0] == '$' and topic[0] != '$') or (topic[0] == '$' and sub[0] != '$'):
                return False

        spos = 0
        tpos = 0

        while spos < slen and tpos < tlen:
            if sub[spos] == topic[tpos]:
                if tpos == tlen - 1:
                    # Check for e.g. foo matching foo/#
                    if spos == slen - 3 and sub[spos + 1] == '/' and sub[spos + 2] == '#':
                        result = True
                        multilevel_wildcard = True
                        break

                spos += 1
                tpos += 1

                if tpos == tlen and spos == slen - 1 and sub[spos] == '+':
                    spos += 1
                    result = True
                    break
            else:
                if sub[spos] == '+':
                    spos += 1
                    while tpos < tlen and topic[tpos] != '/':
                        tpos += 1
                    if tpos == tlen and spos == slen:
                        result = True
                        break

                elif sub[spos] == '#':
                    multilevel_wildcard = True
                    if spos + 1 != slen:
                        result = False
                        break
                    else:
                        result = True
                        break

                else:
                    result = False
                    break

        if not multilevel_wildcard and (tpos < tlen or spos < slen):
            result = False

        return result

