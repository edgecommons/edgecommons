"""The per-instance seam (UNS-CANONICAL-DESIGN §3, D-U3): an instance-scoped handle
whose only job is to pre-bind the instance token into (a) the
:class:`~ggcommons.uns.Uns` topic builder and (b) the
:class:`~ggcommons.messaging.message_builder.MessageBuilder`. The messaging client
stays instance-agnostic — ``publish(topic, msg)`` already receives both the topic
(minted by this handle's instance-bound ``uns()``) and the envelope (stamped by its
instance-bound builder), which is why the seam works unchanged over Python's
static/process-global ``MessagingClient``.

Obtain handles from ``GGCommons.instance(id)`` (validated + cached per id).
Component-level messages (everything not built through a handle) default to instance
``"main"``.
"""
from ggcommons.messaging.message_builder import MessageBuilder
from ggcommons.uns import Uns


class GgInstance:
    """An instance-scoped handle: ``uns()`` mints topics with — and ``new_message()``
    stamps envelopes with — this handle's instance token."""

    def __init__(self, instance_id: str, config_manager, include_root: bool):
        """Library-internal: created by ``GGCommons.instance(id)``, which validates the
        token (§2.2 token rule) and caches per id."""
        self._id = instance_id
        self._config_manager = config_manager
        self._uns = Uns(
            config_manager.get_component_identity().with_instance(instance_id),
            include_root,
        )

    def id(self) -> str:
        """This handle's instance token."""
        return self._id

    def uns(self) -> Uns:
        """The topic builder bound to this instance (topics minted with this instance
        token)."""
        return self._uns

    def new_message(self, name: str, version: str) -> MessageBuilder:
        """Starts a message pre-bound to this instance — equivalent to
        ``MessageBuilder.create(name, version).with_config(config).with_instance(id())``,
        so ``build()`` stamps the component identity with this handle's instance
        token."""
        return (
            MessageBuilder.create(name, version)
            .with_config(self._config_manager)
            .with_instance(self._id)
        )
