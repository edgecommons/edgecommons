package <<PACKAGE>>;

/**
 * Why a delivery failed — and, crucially, <b>whether retrying could ever help</b>.
 *
 * <p>Getting this classification wrong is expensive in both directions: retrying a permanent failure
 * burns the retry budget and floods the log, while giving up on a transient one loses data that a
 * second attempt would have delivered.
 *
 * <p>When in doubt, prefer {@link #transientFailure(String, Throwable) transient}: a
 * wrongly-transient failure wastes retries; a wrongly-permanent one loses data.
 */
public final class DeliverException extends Exception {

    private static final long serialVersionUID = 1L;

    private final boolean isTransient;

    private DeliverException(boolean isTransient, String message, Throwable cause) {
        super(message, cause);
        this.isTransient = isTransient;
    }

    /** The world may differ next time: a timeout, a 503, a full disk that someone will empty. */
    public static DeliverException transientFailure(String message, Throwable cause) {
        return new DeliverException(true, message, cause);
    }

    /** As above, with no underlying cause. */
    public static DeliverException transientFailure(String message) {
        return new DeliverException(true, message, null);
    }

    /** It will fail identically forever: bad credentials, a malformed key, a missing bucket. */
    public static DeliverException permanentFailure(String message, Throwable cause) {
        return new DeliverException(false, message, cause);
    }

    /** As above, with no underlying cause. */
    public static DeliverException permanentFailure(String message) {
        return new DeliverException(false, message, null);
    }

    /** {@code true} when a later attempt could plausibly succeed. */
    public boolean isTransient() {
        return isTransient;
    }
}
