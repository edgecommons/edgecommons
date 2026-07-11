package <<PACKAGE>>;

/**
 * Proof of what landed: returned by {@link Destination#deliver(Item)} and checked by
 * {@link Destination#verify(Item, Delivered)}.
 *
 * <p>A richer destination returns more here — an ETag, a checksum, a version id. Whatever it is, it
 * must be something {@code verify} can compare against reality, because "deliver returned without
 * throwing" is not evidence that the right bytes are at the right key.
 *
 * @param bytesWritten how many bytes the destination says it wrote
 */
public record Delivered(long bytesWritten) {
}
