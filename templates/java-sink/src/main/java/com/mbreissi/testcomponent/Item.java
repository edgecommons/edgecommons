package <<PACKAGE>>;

/**
 * One unit of work to deliver: an opaque payload plus the <b>stable key</b> it belongs at.
 *
 * <p>The key is deterministic — the same item always lands at the same place, so a redelivery is an
 * <b>idempotent overwrite</b> rather than a duplicate. That is what makes retry safe: a sink that
 * cannot retry without duplicating cannot retry at all.
 *
 * @param key   the stable, deterministic destination key
 * @param bytes the payload, exactly as it should land
 */
public record Item(String key, byte[] bytes) {
}
