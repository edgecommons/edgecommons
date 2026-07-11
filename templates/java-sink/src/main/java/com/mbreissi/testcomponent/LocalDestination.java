package <<PACKAGE>>;

import java.io.IOException;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * A local-filesystem destination.
 *
 * <p>Small, but it demonstrates the two things every destination must get right:
 *
 * <ul>
 *   <li><b>Write to a temp file and rename.</b> A rename within a filesystem is atomic, so a reader
 *       never observes a half-written object, and a crash mid-write leaves no corrupt artifact at
 *       the real key.</li>
 *   <li><b>Derive the key deterministically</b> (the caller's job — see {@link Item}), so a
 *       redelivery <b>overwrites</b> rather than duplicating.</li>
 * </ul>
 */
public final class LocalDestination implements Destination {

    private final Path root;

    public LocalDestination(Path root) {
        this.root = root;
    }

    /** The root directory delivered objects land under. */
    public Path root() {
        return root;
    }

    @Override
    public String kind() {
        return "local";
    }

    @Override
    public Delivered deliver(Item item) throws DeliverException {
        Path finalPath = root.resolve(item.key());
        Path parent = finalPath.getParent() != null ? finalPath.getParent() : root;

        try {
            // A directory we cannot create is usually a permission or a path problem, and those do
            // not fix themselves — but a full disk does. Transient is the safer default: a
            // wrongly-transient failure wastes retries, a wrongly-permanent one loses data.
            Files.createDirectories(parent);
        } catch (IOException e) {
            throw DeliverException.transientFailure("creating the destination directory", e);
        }

        Path tmp = parent.resolve("." + sanitize(item.key()) + ".partial");
        try {
            Files.write(tmp, item.bytes());
        } catch (IOException e) {
            throw DeliverException.transientFailure("writing the temp file", e);
        }

        // The atomic step. Until this returns, nothing exists at the real key.
        try {
            move(tmp, finalPath);
        } catch (IOException e) {
            deleteQuietly(tmp);
            throw DeliverException.transientFailure("renaming into place", e);
        }

        return new Delivered(item.bytes().length);
    }

    @Override
    public void verify(Item item, Delivered delivered) throws DeliverException {
        Path path = root.resolve(item.key());
        long landed;
        try {
            landed = Files.size(path);
        } catch (IOException e) {
            throw DeliverException.transientFailure("stat-ing the delivered object", e);
        }
        if (landed != delivered.bytesWritten()) {
            // The object is there but wrong. Do NOT release the source.
            throw DeliverException.transientFailure(
                    "size mismatch: wrote " + delivered.bytesWritten() + " bytes, found " + landed);
        }
    }

    /**
     * ATOMIC_MOVE where the filesystem supports it (every ordinary same-volume rename), falling back
     * to a plain replacing move where it does not.
     */
    private static void move(Path tmp, Path finalPath) throws IOException {
        try {
            Files.move(tmp, finalPath,
                    StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        } catch (AtomicMoveNotSupportedException e) {
            Files.move(tmp, finalPath, StandardCopyOption.REPLACE_EXISTING);
        }
    }

    private static void deleteQuietly(Path p) {
        try {
            Files.deleteIfExists(p);
        } catch (IOException ignored) {
            // Best effort: a leftover .partial is inert, and the delivery failure is what matters.
        }
    }

    /** Keeps a temp-file name from escaping its directory. */
    static String sanitize(String key) {
        return key.replace('/', '_').replace('\\', '_');
    }
}
