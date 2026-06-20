/**
 * GGCommons TypeScript library (spike) — public surface.
 *
 * Mirrors the Java/Python/Rust libraries' messaging core: the cross-language
 * {@link Message} envelope and a STANDALONE-mode MQTT {@link StandaloneProvider}.
 */
export {
  Message,
  MessageBuilder,
  MessageHeader,
  MessageTags,
} from "./message";
export {
  StandaloneProvider,
  StandaloneOptions,
  MessageHandler,
  REPLY_TOPIC_PREFIX,
  topicMatches,
} from "./standalone";
