/**
 * Messaging subsystem — public surface.
 *
 * The transport/service split (mirroring Rust): {@link MessagingProvider}
 * implementations ({@link StandaloneMqttProvider}, {@link IpcMessagingProvider})
 * carry raw bytes; {@link DefaultMessagingService} adds the message envelope,
 * dispatch, and request/reply on top.
 */
export {
  MAX_BINARY_BODY_BYTES,
  Message,
  MessageBodyCase,
  MessageBuilder,
  MessageIdentity,
} from "../message";
export type { MessageBodySchema, MessageHeader, MessageTags, HierLevel } from "../message";
export {
  Destination,
  Qos,
  ReplyFuture,
  RequestTimeoutError,
  ReservedTopicError,
  REPLY_TOPIC_PREFIX,
} from "./types";
export type {
  MessageHandler,
  MessagingProvider,
  IMessagingService,
  RawSubscription,
} from "./types";
export { DefaultMessagingService } from "./service";
export { StandaloneMqttProvider, topicMatches } from "./standalone-provider";
export { IpcMessagingProvider } from "./ipc-provider";
export type { IpcOptions } from "./ipc-provider";
export {
  loadMessagingConfig,
  resolvedHost,
} from "./config";
export type { MessagingConfig, BrokerConfig, Credentials } from "./config";
