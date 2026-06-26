/**
 * Parameter sources (the pluggable seam) — TypeScript port of the Rust `parameters::source`.
 *
 * A {@link ParameterSource} is the backend the parameter service reads from — AWS SSM (cloud), a
 * mounted directory (K8s ConfigMap/Secret volumes, Docker secrets), env vars, or a custom
 * host-supplied source. The service (cache, refresh, typed reads) is identical regardless of source.
 */
import { readFileSync, readdirSync, statSync } from "fs";
import { join } from "path";

import { ParameterError } from "./errors";

/**
 * A parameter value fetched from a source. `secure` values (SSM SecureString, a `mountedDir`
 * secret path, …) must never be logged.
 */
export interface ParamValue {
  /** Raw value bytes (UTF-8 for SSM / env / text files). */
  value: Buffer;
  /** Whether this value is sensitive (don't log; cache encrypted). */
  secure: boolean;
  /** Upstream version, for change detection on refresh (`undefined` if the source has none). */
  version?: string;
}

/** Construct a non-secure {@link ParamValue}. */
export function plainValue(value: Buffer): ParamValue {
  return { value, secure: false };
}

/**
 * True for kubelet/Docker volume-projection artifacts and hidden entries — anything whose file name
 * begins with `"."`. This is the single source of truth for the dotfile filter that skips the kubelet
 * symlink farm (`..data`, `..2026_06_25_…` timestamped dirs, and the `..data_tmp` swap-staging entry).
 * Reused by the `CONFIGMAP` config source so the filter stays identical across the parameters and
 * config subsystems (FR-CFG-4).
 *
 * @param fileName the bare file name (not a path)
 * @returns `true` if the entry is a projection artifact / hidden file to ignore
 */
export function isProjectionArtifact(fileName: string): boolean {
  return fileName.startsWith(".");
}

/** The pluggable parameter backend. */
export interface ParameterSource {
  /** Fetch one parameter by name, or `undefined` if it does not exist. */
  fetch(name: string): Promise<ParamValue | undefined>;
  /** Fetch every parameter under `path` (recursively when `recursive`). Empty when absent. */
  fetchByPath(path: string, recursive: boolean): Promise<Array<[string, ParamValue]>>;
  /** Stable id for diagnostics/stats (e.g. `"awsSsm"`, `"mountedDir"`, `"env"`). */
  sourceId(): string;
}

// ---------------------------------------------------------------------------
// EnvSource — parameters from environment variables (containers / dev / STANDALONE).
// ---------------------------------------------------------------------------

/**
 * Reads parameters from environment variables under a prefix. A name `/myapp/db/host` maps to the
 * env var `<PREFIX>MYAPP_DB_HOST` and back. Values are treated as non-secure (env is plaintext).
 */
export class EnvSource implements ParameterSource {
  constructor(private readonly prefix: string) {}

  /** Map a parameter name to its env-var name. */
  private toEnv(name: string): string {
    const body = name
      .replace(/^\/+/, "")
      .split("")
      .map((c) => (c === "/" || c === "-" || c === "." ? "_" : c.toUpperCase()))
      .join("");
    return `${this.prefix}${body}`;
  }

  /** Map an env-var name back to a parameter name (lossy: `_` -> `/`). */
  private fromEnv(varName: string): string | undefined {
    if (!varName.startsWith(this.prefix)) return undefined;
    const rest = varName.slice(this.prefix.length);
    return `/${rest.toLowerCase().replace(/_/g, "/")}`;
  }

  async fetch(name: string): Promise<ParamValue | undefined> {
    const v = process.env[this.toEnv(name)];
    return v === undefined ? undefined : plainValue(Buffer.from(v, "utf-8"));
  }

  async fetchByPath(path: string, _recursive: boolean): Promise<Array<[string, ParamValue]>> {
    const out: Array<[string, ParamValue]> = [];
    for (const [k, v] of Object.entries(process.env)) {
      if (v === undefined) continue;
      const name = this.fromEnv(k);
      if (name && name.startsWith(path)) {
        out.push([name, plainValue(Buffer.from(v, "utf-8"))]);
      }
    }
    return out;
  }

  sourceId(): string {
    return "env";
  }
}

// ---------------------------------------------------------------------------
// MountedDirSource — parameters from a directory tree (K8s ConfigMap/Secret volumes,
// Docker secrets at /run/secrets, bare config dirs). No API client / RBAC needed.
// ---------------------------------------------------------------------------

/**
 * Reads parameters from files under a root directory: a file at `<root>/myapp/db/host` is the
 * parameter `/myapp/db/host` with the file's bytes as its value. Files whose parameter name falls
 * under one of `securePaths` are flagged `secure` (a K8s Secret volume vs a ConfigMap volume).
 */
export class MountedDirSource implements ParameterSource {
  constructor(
    private readonly root: string,
    private readonly securePaths: string[],
  ) {}

  private isSecure(name: string): boolean {
    return this.securePaths.some((p) => name.startsWith(p));
  }

  private nameToPath(name: string): string {
    return join(this.root, name.replace(/^\/+/, ""));
  }

  /**
   * Recursively collect files under `dir` into `out`, keyed by parameter name (relative to root,
   * `/`-separated). Skips dotfiles/dirs — K8s projects volumes with internal `..data` /
   * `..2025_…` symlinked entries that must not be surfaced as parameters.
   */
  private walk(dir: string, recursive: boolean, out: Array<[string, ParamValue]>): void {
    let entries: import("fs").Dirent[];
    try {
      entries = readdirSync(dir, { withFileTypes: true });
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return;
      throw new ParameterError(`read dir ${dir}: ${(e as Error).message}`);
    }
    for (const entry of entries) {
      const fname = entry.name;
      if (isProjectionArtifact(fname)) {
        continue; // K8s internal (..data, ..2025_...) / hidden
      }
      const path = join(dir, fname);
      if (entry.isDirectory()) {
        if (recursive) this.walk(path, recursive, out);
      } else if (entry.isFile()) {
        // Parameter name = "/" + path relative to root, with platform separators normalized.
        let rel = path.startsWith(this.root) ? path.slice(this.root.length) : path;
        rel = rel.replace(/^[\\/]+/, "");
        const name = `/${rel.split(/[\\/]/).join("/")}`;
        let value: Buffer;
        try {
          value = readFileSync(path);
        } catch (e) {
          throw new ParameterError(`read ${path}: ${(e as Error).message}`);
        }
        out.push([name, { value, secure: this.isSecure(name) }]);
      }
    }
  }

  async fetch(name: string): Promise<ParamValue | undefined> {
    const path = this.nameToPath(name);
    try {
      // A directory (not a file) at that name is "not a parameter".
      if (statSync(path).isDirectory()) return undefined;
      const value = readFileSync(path);
      return { value, secure: this.isSecure(name) };
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === "ENOENT") return undefined;
      throw new ParameterError(`read ${path}: ${(e as Error).message}`);
    }
  }

  async fetchByPath(path: string, recursive: boolean): Promise<Array<[string, ParamValue]>> {
    const base = this.nameToPath(path);
    const out: Array<[string, ParamValue]> = [];
    this.walk(base, recursive, out);
    return out;
  }

  sourceId(): string {
    return "mountedDir";
  }
}
