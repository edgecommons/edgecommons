/**
 * # The destination: what a *sink* delivers to
 *
 * A sink consumes work and hands it to somewhere outside EdgeCommons — a filesystem, an object
 * store, an HTTP endpoint, a database. {@link Destination} is the seam. Implement it once per
 * backend; everything above it (retry, verification, reporting) is written against the interface
 * and never learns what a bucket is.
 *
 * ## The contract, and why each clause is there
 *
 * * **`deliver` is the commit.** When it resolves, the item is live at its final, *stable* key. Not
 *   staged, not pending — live.
 * * **The key is deterministic.** The same item always lands at the same place, so a redelivery is
 *   an **idempotent overwrite** rather than a duplicate. This is what makes retry safe: a sink that
 *   cannot retry without duplicating cannot retry at all.
 * * **`verify` runs before the source is released.** The whole point of a sink is that it is the
 *   last thing standing between data and its destination. Releasing the source because `deliver`
 *   resolved — without checking that what landed is what you sent — is how you lose the only copy.
 */
import { promises as fs } from "node:fs";
import * as path from "node:path";

/** One unit of work to deliver: an opaque payload plus the stable key it belongs at. */
export interface Item {
  /** The stable, deterministic key. Redelivering the same item overwrites in place. */
  readonly key: string;
  readonly bytes: Buffer;
}

/** Proof of what landed, returned by {@link Destination.deliver} and checked by {@link Destination.verify}. */
export interface Delivered {
  readonly bytesWritten: number;
}

/**
 * Why a delivery failed — and, crucially, **whether retrying could ever help**.
 *
 * Getting this wrong is expensive in both directions: retrying a permanent failure burns the budget
 * and floods the log; giving up on a transient one loses data that a second attempt would have
 * delivered.
 */
export class DeliverError extends Error {
  constructor(
    message: string,
    /** `true` when the world may differ next time (a timeout, a 503, a full disk someone will empty). */
    readonly transient: boolean,
  ) {
    super(message);
    this.name = "DeliverError";
  }

  /** The world may differ next time. Retry. */
  static transientError(message: string): DeliverError {
    return new DeliverError(message, true);
  }

  /** It will fail identically forever: bad credentials, a malformed key, a missing bucket. */
  static permanent(message: string): DeliverError {
    return new DeliverError(message, false);
  }

  /** An unclassified throw is treated as transient — see {@link isTransient}. */
  static isTransient(e: unknown): boolean {
    return e instanceof DeliverError ? e.transient : true;
  }
}

/** A place a sink delivers to. **This is the interface you implement.** */
export interface Destination {
  /** Its kind, as named in config (`local`, `s3`, …). */
  readonly kind: string;

  /** Deliver the item to its stable key. Resolving means it is **live**, not staged. */
  deliver(item: Item): Promise<Delivered>;

  /** Confirm that what landed is what was sent — **before** the source is released. */
  verify(item: Item, delivered: Delivered): Promise<void>;
}

/** The destinations this component understands. Add a variant as you add a backend. */
export type DestinationConfig = { type: "local"; path: string };

/**
 * Build a destination from config.
 *
 * @throws Error when the configured destination cannot be constructed
 */
export function buildDestination(cfg: unknown): Destination {
  if (typeof cfg !== "object" || cfg === null) throw new Error("`destination` must be an object");
  const o = cfg as Record<string, unknown>;

  switch (o.type) {
    case "local": {
      if (typeof o.path !== "string" || o.path === "") {
        throw new Error("a `local` destination needs a `path`");
      }
      for (const key of Object.keys(o)) {
        if (key !== "type" && key !== "path") throw new Error(`unknown key 'destination.${key}'`);
      }
      return new LocalDestination(o.path);
    }
    default:
      throw new Error(`unknown destination type '${String(o.type)}'`);
  }
}

/**
 * A local-filesystem destination.
 *
 * Small, but it demonstrates the two things every destination must get right: **write to a temp file
 * and rename** (a rename is atomic, so a reader never observes a half-written object, and a crash
 * mid-write leaves no corrupt artifact at the real key), and **derive the key deterministically** so
 * a redelivery overwrites rather than duplicates.
 */
export class LocalDestination implements Destination {
  readonly kind = "local";

  constructor(private readonly root: string) {}

  async deliver(item: Item): Promise<Delivered> {
    const finalPath = path.join(this.root, item.key);
    const parent = path.dirname(finalPath);

    try {
      // A directory we cannot create is usually a permission or a path problem, and those do not
      // fix themselves — but a full disk does. Transient is the safer default: a wrongly-transient
      // failure wastes retries, a wrongly-permanent one loses data.
      await fs.mkdir(parent, { recursive: true });
    } catch (e) {
      throw DeliverError.transientError(`creating the destination directory: ${String(e)}`);
    }

    const tmp = path.join(parent, `.${sanitize(item.key)}.partial`);
    try {
      await fs.writeFile(tmp, item.bytes);
      // The atomic step. Until this resolves, nothing exists at the real key.
      await fs.rename(tmp, finalPath);
    } catch (e) {
      await fs.rm(tmp, { force: true }).catch(() => undefined);
      throw DeliverError.transientError(`writing the object: ${String(e)}`);
    }

    return { bytesWritten: item.bytes.length };
  }

  async verify(item: Item, delivered: Delivered): Promise<void> {
    const target = path.join(this.root, item.key);
    let landed: number;
    try {
      landed = (await fs.stat(target)).size;
    } catch (e) {
      throw DeliverError.transientError(`stat-ing the delivered object: ${String(e)}`);
    }

    if (landed !== delivered.bytesWritten) {
      // The object is there but wrong. Do NOT release the source.
      throw DeliverError.transientError(
        `size mismatch: wrote ${delivered.bytesWritten} bytes, found ${landed}`,
      );
    }
  }
}

/** Keep a temp-file name from escaping its directory. */
function sanitize(key: string): string {
  return key.replace(/[/\\]/g, "_");
}
