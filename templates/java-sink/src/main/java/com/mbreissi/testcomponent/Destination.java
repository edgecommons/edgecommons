package <<PACKAGE>>;

import com.google.gson.JsonObject;

/**
 * The destination: what a <i>sink</i> delivers to.
 *
 * <p>A sink consumes work and hands it to somewhere outside EdgeCommons — a filesystem, an object
 * store, an HTTP endpoint, a database. This interface is the seam. Implement it once per backend;
 * everything above it (retry, verification, reporting) is written against the interface and never
 * learns what a bucket is.
 *
 * <h2>The contract, and why each clause is there</h2>
 *
 * <ul>
 *   <li><b>{@code deliver} is the commit.</b> When it returns, the item is live at its final,
 *       <i>stable</i> key. Not staged, not pending — live.</li>
 *   <li><b>The key is deterministic.</b> The same item always lands at the same place, so a
 *       redelivery is an <b>idempotent overwrite</b> rather than a duplicate. This is what makes
 *       retry safe: a sink that cannot retry without duplicating cannot retry at all.</li>
 *   <li><b>{@code verify} runs before the source is released.</b> The whole point of a sink is that
 *       it is the last thing standing between data and its destination. Releasing the source
 *       because {@code deliver} returned — without checking that what landed is what you sent — is
 *       how you lose the only copy.</li>
 * </ul>
 */
public interface Destination {

    /** Its kind, as named in config ({@code local}, {@code s3}, …). */
    String kind();

    /**
     * Delivers the item to its stable key. Returning normally means it is <b>live</b>, not staged.
     *
     * @param item the item to deliver
     * @return proof of what landed, for {@link #verify(Item, Delivered)} to check
     * @throws DeliverException classified transient or permanent — see {@link DeliverException}
     */
    Delivered deliver(Item item) throws DeliverException;

    /**
     * Confirms that what landed is what was sent — <b>before</b> the source is released.
     *
     * @param item      the item that was delivered
     * @param delivered what {@link #deliver(Item)} claimed
     * @throws DeliverException when the destination does not hold what it should
     */
    void verify(Item item, Delivered delivered) throws DeliverException;

    /**
     * Builds a destination from its config object. <b>Add a case here as you add a backend</b> (and
     * a branch to {@code config.schema.json}'s {@code $defs/destination}).
     *
     * @param cfg the tagged {@code destination} object: {@code {"type": "local", "path": "…"}}
     * @return the destination
     * @throws IllegalArgumentException when the type is unknown or its arguments are missing
     */
    static Destination build(JsonObject cfg) {
        String type = cfg.has("type") ? cfg.get("type").getAsString() : "";
        return switch (type) {
            case "local" -> {
                if (!cfg.has("path")) {
                    throw new IllegalArgumentException("destination `local` requires a `path`");
                }
                yield new LocalDestination(java.nio.file.Path.of(cfg.get("path").getAsString()));
            }
            default -> throw new IllegalArgumentException("unknown destination type `" + type + "`");
        };
    }
}
