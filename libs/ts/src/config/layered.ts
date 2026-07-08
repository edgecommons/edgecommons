import * as fs from "fs";
import * as fsp from "fs/promises";
import * as path from "path";

import { ConfigSourceSpec } from "../cli";
import { EdgeCommonsError } from "../errors";
import { IpcMessagingProvider } from "../messaging/ipc-provider";
import { Destination, Qos } from "../messaging/types";
import { logger } from "../logging";
import { isProjectionArtifact } from "../parameters/source";
import { ConfigSource, ConfigWatch } from "./source";
import { ConfigMapConfigSource } from "./source/configmap";
import { cloneJson, deepMerge, isJsonObject, JsonObject } from "./merge";

const SHARED_CONFIG_ENV = "EDGECOMMONS_SHARED_CONFIG";
const SHARED_COMPONENT_ENV = "EDGECOMMONS_SHARED_COMPONENT";
const DEFAULT_FILE_SHARED_PATH = "/etc/edgecommons/shared.json";
const DEFAULT_SHARED_COMPONENT = "com.mbreissi.edgecommons.EdgeCommonsSharedConfig";
const SHARED_GG_CONFIG_KEY = "SharedComponentConfig";
const SHARED_SHADOW_NAME = "edgecommons-shared";
const SHADOW_CONFIG_KEY = "ComponentConfig";

export interface BuildBaseLayerResolverOptions {
  ipcProvider?: IpcMessagingProvider;
  thingName: string;
  componentName: string;
}

export interface BaseLayerResolver {
  sourceName(): string;
  resolve(componentLayer: JsonObject): Promise<JsonObject | undefined>;
  watch(
    componentLayer: JsonObject,
    onUpdate: (baseLayer: JsonObject | undefined) => void,
  ): Promise<ConfigWatch | undefined>;
}

export interface LayeredConfigCoordinatorOptions {
  source: ConfigSource;
  sourceSpec: ConfigSourceSpec;
  baseResolver?: BaseLayerResolver;
  noSharedConfig: boolean;
}

interface LayerCandidate {
  componentLayer: JsonObject;
  baseLayer?: JsonObject;
  effective: JsonObject;
  sharedDisabled: boolean;
}

export class LayeredConfigCoordinator {
  private latestComponentLayer?: JsonObject;
  private latestBaseLayer?: JsonObject;
  private latestEffective?: JsonObject;
  private sourceWatch?: ConfigWatch;
  private baseWatch?: ConfigWatch;
  private applyEffective?: (effective: JsonObject) => boolean;

  constructor(private readonly opts: LayeredConfigCoordinatorOptions) {}

  async loadEffective(): Promise<JsonObject> {
    const raw = await this.opts.source.load();
    const candidate = await this.candidateFromSource(raw, false);
    this.commit(candidate);
    this.logSharedStatus(candidate);
    return cloneJson(candidate.effective);
  }

  async reloadFromProvider(applyEffective: (effective: JsonObject) => boolean): Promise<boolean> {
    let raw: unknown;
    try {
      raw = await this.opts.source.load();
    } catch (e) {
      logger.warn(
        `reload-config: re-fetch from the '${this.opts.source.sourceName()}' source failed: ${String(e)}`,
      );
      return false;
    }
    return this.applySourceUpdate(raw, applyEffective, false);
  }

  async watch(applyEffective: (effective: JsonObject) => boolean): Promise<ConfigWatch | undefined> {
    this.applyEffective = applyEffective;
    this.sourceWatch = await this.opts.source.watch((raw) => {
      void this.applySourceUpdate(raw, applyEffective, true);
    });
    await this.rearmBaseWatch();
    if (!this.sourceWatch && !this.baseWatch) return undefined;
    return {
      close: async () => {
        await this.sourceWatch?.close().catch(() => undefined);
        await this.baseWatch?.close().catch(() => undefined);
        this.sourceWatch = undefined;
        this.baseWatch = undefined;
      },
    };
  }

  private async applySourceUpdate(
    raw: unknown,
    applyEffective: (effective: JsonObject) => boolean,
    preserveLegacyBase: boolean,
  ): Promise<boolean> {
    let candidate: LayerCandidate;
    try {
      candidate = await this.candidateFromSource(raw, preserveLegacyBase);
    } catch (e) {
      logger.warn(`reloaded config layers rejected; keeping previous: ${String(e)}`);
      return false;
    }
    if (!applyEffective(candidate.effective)) {
      return false;
    }
    this.commit(candidate);
    await this.rearmBaseWatch();
    return true;
  }

  private applyBaseUpdate(baseLayer: JsonObject | undefined, applyEffective: (effective: JsonObject) => boolean): boolean {
    if (!this.latestComponentLayer) return false;
    let candidate: LayerCandidate;
    try {
      const controls = readComponentControls(this.latestComponentLayer);
      const sharedDisabled = this.opts.noSharedConfig || !controls.sharedEnabled;
      candidate = this.mergeCandidate(
        this.latestComponentLayer,
        sharedDisabled ? undefined : baseLayer,
        sharedDisabled,
      );
    } catch (e) {
      logger.warn(`reloaded shared config rejected; keeping previous: ${String(e)}`);
      return false;
    }
    if (!applyEffective(candidate.effective)) {
      return false;
    }
    this.commit(candidate);
    return true;
  }

  private async candidateFromSource(raw: unknown, preserveLegacyBase: boolean): Promise<LayerCandidate> {
    const layers =
      this.opts.sourceSpec.kind === "CONFIG_COMPONENT"
        ? parseConfigComponentPayload(raw)
        : { basePresent: false, componentLayer: requireObject(raw, "component config layer") };
    const controls = readComponentControls(layers.componentLayer);
    const sharedDisabled = this.opts.noSharedConfig || !controls.sharedEnabled;
    const configComponentBase =
      this.opts.sourceSpec.kind !== "CONFIG_COMPONENT"
        ? undefined
        : layers.basePresent
          ? layers.baseLayer
          : preserveLegacyBase
            ? this.latestBaseLayer
            : undefined;
    const baseLayer =
      sharedDisabled || this.opts.sourceSpec.kind === "CONFIG_COMPONENT"
        ? undefined
        : await this.opts.baseResolver?.resolve(layers.componentLayer);
    const bundledBase =
      sharedDisabled || this.opts.sourceSpec.kind !== "CONFIG_COMPONENT" ? undefined : configComponentBase;
    return this.mergeCandidate(layers.componentLayer, baseLayer ?? bundledBase, sharedDisabled);
  }

  private mergeCandidate(
    componentLayer: JsonObject,
    baseLayer: JsonObject | undefined,
    sharedDisabled: boolean,
  ): LayerCandidate {
    if (baseLayer) ensureBaseLayerAllowed(baseLayer);
    const layers = baseLayer ? [baseLayer, componentLayer] : [componentLayer];
    const merged = deepMerge(layers).effective;
    return {
      componentLayer: cloneJson(componentLayer),
      baseLayer: baseLayer ? cloneJson(baseLayer) : undefined,
      effective: merged,
      sharedDisabled,
    };
  }

  private commit(candidate: LayerCandidate): void {
    this.latestComponentLayer = cloneJson(candidate.componentLayer);
    this.latestBaseLayer = candidate.baseLayer ? cloneJson(candidate.baseLayer) : undefined;
    this.latestEffective = cloneJson(candidate.effective);
  }

  private async rearmBaseWatch(): Promise<void> {
    await this.baseWatch?.close().catch(() => undefined);
    this.baseWatch = undefined;
    if (!this.latestComponentLayer || !this.opts.baseResolver || !this.applyEffective) return;
    const controls = readComponentControls(this.latestComponentLayer);
    if (this.opts.noSharedConfig || !controls.sharedEnabled) return;
    this.baseWatch = await this.opts.baseResolver.watch(this.latestComponentLayer, (baseLayer) => {
      if (this.applyEffective) {
        this.applyBaseUpdate(baseLayer, this.applyEffective);
      }
    });
  }

  private logSharedStatus(candidate: LayerCandidate): void {
    if (candidate.sharedDisabled) {
      logger.info("shared config disabled");
    } else if (candidate.baseLayer) {
      logger.info(`shared config applied from ${this.sharedSourceName()}`);
    } else {
      logger.info(`shared config absent for ${this.sharedSourceName()}`);
    }
  }

  private sharedSourceName(): string {
    return this.opts.sourceSpec.kind === "CONFIG_COMPONENT"
      ? "CONFIG_COMPONENT bundle"
      : this.opts.baseResolver?.sourceName() ?? this.opts.source.sourceName();
  }
}

export function buildBaseLayerResolver(
  spec: ConfigSourceSpec,
  opts: BuildBaseLayerResolverOptions,
): BaseLayerResolver | undefined {
  switch (spec.kind) {
    case "FILE":
      return new FileBaseLayerResolver(spec.path);
    case "CONFIGMAP":
      return new ConfigMapBaseLayerResolver(
        spec.mountDir ?? ConfigMapConfigSource.DEFAULT_MOUNT_DIR,
        spec.key ?? ConfigMapConfigSource.DEFAULT_KEY,
      );
    case "ENV":
      return new EnvBaseLayerResolver();
    case "GG_CONFIG":
      if (!opts.ipcProvider) return undefined;
      return new GreengrassBaseLayerResolver(opts.ipcProvider);
    case "SHADOW":
      if (!opts.ipcProvider) return undefined;
      return new ShadowBaseLayerResolver(opts.ipcProvider, opts.thingName);
    case "CONFIG_COMPONENT":
      return undefined;
    default: {
      const _exhaustive: never = spec;
      return _exhaustive;
    }
  }
}

export function parseConfigComponentPayload(raw: unknown): {
  baseLayer?: JsonObject;
  basePresent: boolean;
  componentLayer: JsonObject;
} {
  const body = requireObject(raw, "CONFIG_COMPONENT payload");
  const ok = body.ok;
  const error = body.error;
  if (ok === false && isJsonObject(error)) {
    const code = typeof error.code === "string" ? error.code : "CONFIG_COMPONENT_ERROR";
    const message = typeof error.message === "string" ? error.message : "CONFIG_COMPONENT server error";
    throw EdgeCommonsError.config(`${code}: ${message}`);
  }
  if (Object.prototype.hasOwnProperty.call(body, "base")) {
    const base = body.base;
    const component = body.component;
    if (base !== null && base !== undefined && !isJsonObject(base)) {
      throw EdgeCommonsError.config("CONFIG_COMPONENT_BUNDLE_INVALID: base must be an object or null");
    }
    if (!isJsonObject(component)) {
      throw EdgeCommonsError.config("CONFIG_COMPONENT_BUNDLE_INVALID: component must be an object");
    }
    return {
      baseLayer: base === null || base === undefined ? undefined : cloneJson(base),
      basePresent: true,
      componentLayer: cloneJson(component),
    };
  }
  return { basePresent: false, componentLayer: cloneJson(body) };
}

export function readComponentControls(componentLayer: JsonObject): {
  sharedEnabled: boolean;
  extendsPath?: string;
} {
  const sharedConfig = componentLayer.sharedConfig;
  if (sharedConfig !== undefined && typeof sharedConfig !== "boolean") {
    throw EdgeCommonsError.config("sharedConfig must be a boolean when present");
  }
  const ext = componentLayer.extends;
  if (ext !== undefined && (typeof ext !== "string" || ext.trim() === "")) {
    throw EdgeCommonsError.config("extends must be a non-empty string when present");
  }
  return {
    sharedEnabled: sharedConfig !== false,
    extendsPath: typeof ext === "string" ? ext : undefined,
  };
}

export function ensureBaseLayerAllowed(baseLayer: JsonObject): void {
  if (Object.prototype.hasOwnProperty.call(baseLayer, "extends")) {
    throw EdgeCommonsError.config("N_LAYER_INHERITANCE_NOT_IMPLEMENTED: shared layer extends is not implemented");
  }
}

export function resolveFileBasePath(componentPath: string, componentLayer: JsonObject): {
  path: string;
  missingIsNoop: boolean;
} {
  const controls = readComponentControls(componentLayer);
  if (controls.extendsPath) {
    return {
      path: resolveSiblingPath(componentPath, controls.extendsPath),
      missingIsNoop: false,
    };
  }
  const envPath = process.env[SHARED_CONFIG_ENV];
  if (envPath !== undefined) {
    return { path: envPath, missingIsNoop: false };
  }
  return { path: DEFAULT_FILE_SHARED_PATH, missingIsNoop: true };
}

export function resolveConfigMapBasePath(
  mountDir: string,
  componentPath: string,
  componentLayer: JsonObject,
): {
  path: string;
  missingIsNoop: boolean;
} {
  const controls = readComponentControls(componentLayer);
  if (controls.extendsPath) {
    const resolved = resolveSiblingPath(componentPath, controls.extendsPath);
    if (isProjectionArtifact(path.basename(resolved))) {
      throw EdgeCommonsError.config(`ConfigMap shared key must not be a kubelet projection artifact: ${resolved}`);
    }
    return { path: resolved, missingIsNoop: false };
  }
  const envPath = process.env[SHARED_CONFIG_ENV];
  if (envPath !== undefined) {
    return { path: envPath, missingIsNoop: false };
  }
  return { path: path.join(mountDir, "shared.json"), missingIsNoop: true };
}

class FileBaseLayerResolver implements BaseLayerResolver {
  constructor(private readonly componentPath: string) {}

  sourceName(): string {
    return "FILE shared config";
  }

  async resolve(componentLayer: JsonObject): Promise<JsonObject | undefined> {
    const location = resolveFileBasePath(this.componentPath, componentLayer);
    return readJsonObjectFile(location.path, location.missingIsNoop, "shared config file");
  }

  async watch(
    componentLayer: JsonObject,
    onUpdate: (baseLayer: JsonObject | undefined) => void,
  ): Promise<ConfigWatch | undefined> {
    const location = resolveFileBasePath(this.componentPath, componentLayer);
    if (location.missingIsNoop && !fs.existsSync(location.path)) return undefined;
    return watchJsonObjectFile(location.path, location.missingIsNoop, "shared config file", onUpdate);
  }
}

class ConfigMapBaseLayerResolver implements BaseLayerResolver {
  private static readonly REARM_BACKOFF_MS = 200;
  private readonly componentPath: string;

  constructor(
    private readonly mountDir: string,
    private readonly key: string,
  ) {
    this.componentPath = path.join(mountDir, key);
  }

  sourceName(): string {
    return "CONFIGMAP shared config";
  }

  async resolve(componentLayer: JsonObject): Promise<JsonObject | undefined> {
    const location = resolveConfigMapBasePath(this.mountDir, this.componentPath, componentLayer);
    return readJsonObjectFile(location.path, location.missingIsNoop, "ConfigMap shared config");
  }

  async watch(
    componentLayer: JsonObject,
    onUpdate: (baseLayer: JsonObject | undefined) => void,
  ): Promise<ConfigWatch | undefined> {
    let closed = false;
    let watcher: fs.FSWatcher | undefined;
    let rearmTimer: NodeJS.Timeout | undefined;

    const reload = (): void => {
      void this.resolve(componentLayer)
        .then((base) => {
          if (!closed) onUpdate(base);
        })
        .catch((e) => logger.warn(`ConfigMap shared config reload rejected: ${String(e)}`));
    };

    const scheduleRearm = (): void => {
      if (closed) return;
      rearmTimer = setTimeout(arm, ConfigMapBaseLayerResolver.REARM_BACKOFF_MS);
      if (typeof rearmTimer.unref === "function") rearmTimer.unref();
    };

    const arm = (): void => {
      if (closed) return;
      try {
        watcher = fs.watch(this.mountDir, { persistent: false }, () => {
          reload();
        });
        watcher.on("error", (e) => {
          logger.warn(
            `ConfigMap shared config directory watch on '${this.mountDir}' errored (${(e as Error).message}); re-arming.`,
          );
          try {
            watcher?.close();
          } catch {
            /* ignore */
          }
          watcher = undefined;
          scheduleRearm();
        });
      } catch (e) {
        logger.warn(
          `failed to watch ConfigMap shared config directory '${this.mountDir}' (${(e as Error).message}); retrying.`,
        );
        scheduleRearm();
      }
    };

    arm();
    return {
      close: async () => {
        closed = true;
        if (rearmTimer) clearTimeout(rearmTimer);
        try {
          watcher?.close();
        } catch {
          /* ignore */
        }
      },
    };
  }
}

class EnvBaseLayerResolver implements BaseLayerResolver {
  sourceName(): string {
    return "ENV shared config";
  }

  async resolve(_componentLayer: JsonObject): Promise<JsonObject | undefined> {
    const raw = process.env[SHARED_CONFIG_ENV];
    if (raw === undefined) return undefined;
    if (raw.startsWith("@")) {
      const filePath = raw.slice(1);
      if (filePath.trim() === "") {
        throw EdgeCommonsError.config(`${SHARED_CONFIG_ENV} @path must not be empty`);
      }
      return readJsonObjectFile(filePath, false, "ENV shared config file");
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (e) {
      throw EdgeCommonsError.json(`${SHARED_CONFIG_ENV} does not contain valid JSON: ${(e as Error).message}`);
    }
    return requireObject(parsed, `${SHARED_CONFIG_ENV} shared config`);
  }

  async watch(_componentLayer: JsonObject, _onUpdate: (baseLayer: JsonObject | undefined) => void): Promise<undefined> {
    return undefined;
  }
}

class GreengrassBaseLayerResolver implements BaseLayerResolver {
  constructor(private readonly ipc: IpcMessagingProvider) {}

  sourceName(): string {
    return "GG_CONFIG shared config";
  }

  async resolve(_componentLayer: JsonObject): Promise<JsonObject | undefined> {
    const explicitComponent = process.env[SHARED_COMPONENT_ENV];
    const component = explicitComponent ?? DEFAULT_SHARED_COMPONENT;
    let value: unknown;
    try {
      value = await this.ipc.getConfiguration([SHARED_GG_CONFIG_KEY], component);
    } catch (e) {
      if (explicitComponent === undefined) {
        logger.info(`shared GG_CONFIG unavailable from default component '${component}'; continuing without base`);
        return undefined;
      }
      if (e instanceof EdgeCommonsError) throw e;
      throw EdgeCommonsError.config(`SHARED_CONFIG_UNAVAILABLE: ${(e as Error).message}`);
    }
    if (value === undefined || value === null) {
      if (explicitComponent === undefined) return undefined;
      throw EdgeCommonsError.config("SHARED_CONFIG_UNAVAILABLE");
    }
    return requireObject(value, "GG_CONFIG shared config");
  }

  async watch(
    _componentLayer: JsonObject,
    onUpdate: (baseLayer: JsonObject | undefined) => void,
  ): Promise<ConfigWatch | undefined> {
    const component = process.env[SHARED_COMPONENT_ENV] ?? DEFAULT_SHARED_COMPONENT;
    try {
      const sub = await this.ipc.watchConfiguration([SHARED_GG_CONFIG_KEY], component, async () => {
        try {
          onUpdate(await this.resolve({}));
        } catch (e) {
          logger.warn(`GG_CONFIG shared config reload rejected: ${String(e)}`);
        }
      });
      return {
        close: async () => {
          await sub.unsubscribe();
        },
      };
    } catch (e) {
      logger.warn(`GG_CONFIG shared config watch unavailable: ${(e as Error).message}`);
      return undefined;
    }
  }
}

class ShadowBaseLayerResolver implements BaseLayerResolver {
  constructor(
    private readonly ipc: IpcMessagingProvider,
    private readonly thingName: string,
  ) {}

  sourceName(): string {
    return "SHADOW shared config";
  }

  async resolve(_componentLayer: JsonObject): Promise<JsonObject | undefined> {
    let bytes: Buffer;
    try {
      bytes = await this.ipc.getThingShadow(this.thingName, SHARED_SHADOW_NAME);
    } catch {
      return undefined;
    }
    if (bytes.length === 0) return undefined;
    let doc: unknown;
    try {
      doc = JSON.parse(bytes.toString("utf8"));
    } catch (e) {
      throw EdgeCommonsError.json(`failed to parse shared shadow document: ${(e as Error).message}`);
    }
    const cfg = extractShadowConfig(doc);
    if (cfg === undefined) return undefined;
    return cfg;
  }

  async watch(
    _componentLayer: JsonObject,
    onUpdate: (baseLayer: JsonObject | undefined) => void,
  ): Promise<ConfigWatch | undefined> {
    const filter = `$aws/things/${this.thingName}/shadow/name/${SHARED_SHADOW_NAME}/+/+`;
    const sub = await this.ipc.subscribeRaw(
      filter,
      Destination.Local,
      Qos.AtLeastOnce,
      (_topic: string, payload: Buffer) => {
        let doc: unknown;
        try {
          doc = JSON.parse(payload.toString("utf8"));
        } catch {
          return;
        }
        if (!isJsonObject(doc)) return;
        const state = doc.state;
        if (!isJsonObject(state)) return;
        const value = state[SHADOW_CONFIG_KEY];
        if (value === undefined) return;
        if (typeof value !== "string") {
          logger.warn("SHADOW shared config delta rejected: ComponentConfig must be a string");
          return;
        }
        try {
          onUpdate(requireObject(JSON.parse(value), "SHADOW shared config"));
        } catch (e) {
          logger.warn(`SHADOW shared config delta rejected: ${String(e)}`);
        }
      },
    );
    return {
      close: async () => {
        await sub.unsubscribe();
      },
    };
  }
}

async function readJsonObjectFile(
  filePath: string,
  missingIsNoop: boolean,
  label: string,
): Promise<JsonObject | undefined> {
  let text: string;
  try {
    text = await fsp.readFile(filePath, "utf8");
  } catch (e) {
    const code = (e as NodeJS.ErrnoException).code;
    if (missingIsNoop && code === "ENOENT") return undefined;
    throw EdgeCommonsError.io(`failed to read ${label} '${filePath}': ${(e as Error).message}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (e) {
    throw EdgeCommonsError.config(`failed to parse ${label} '${filePath}': ${(e as Error).message}`);
  }
  return requireObject(parsed, label);
}

function watchJsonObjectFile(
  filePath: string,
  missingIsNoop: boolean,
  label: string,
  onUpdate: (baseLayer: JsonObject | undefined) => void,
): ConfigWatch | undefined {
  const target = path.resolve(filePath);
  const parent = path.dirname(target) || ".";
  const targetName = path.basename(target);
  let watcher: fs.FSWatcher;
  try {
    watcher = fs.watch(parent, { persistent: false }, (_eventType, filename) => {
      if (filename !== null && filename !== undefined && path.basename(filename.toString()) !== targetName) {
        return;
      }
      void readJsonObjectFile(target, missingIsNoop, label)
        .then((base) => onUpdate(base))
        .catch((e) => logger.warn(`shared config reload rejected: ${String(e)}`));
    });
  } catch (e) {
    logger.warn(`failed to watch shared config directory '${parent}': ${(e as Error).message}`);
    return undefined;
  }
  return {
    close: async () => {
      watcher.close();
    },
  };
}

function extractShadowConfig(doc: unknown): JsonObject | undefined {
  if (!isJsonObject(doc)) {
    throw EdgeCommonsError.config("shared shadow document must be an object");
  }
  const state = doc.state;
  if (!isJsonObject(state)) return undefined;
  for (const key of ["desired", "reported"]) {
    const section = state[key];
    if (!isJsonObject(section)) continue;
    const raw = section[SHADOW_CONFIG_KEY];
    if (raw === undefined) continue;
    if (typeof raw !== "string") {
      throw EdgeCommonsError.config("SHADOW shared ComponentConfig must be a string");
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch (e) {
      throw EdgeCommonsError.json(`failed to parse SHADOW shared ComponentConfig: ${(e as Error).message}`);
    }
    return requireObject(parsed, "SHADOW shared config");
  }
  return undefined;
}

function requireObject(value: unknown, label: string): JsonObject {
  if (!isJsonObject(value)) {
    throw EdgeCommonsError.config(`${label} must be a JSON object`);
  }
  return cloneJson(value);
}

function resolveSiblingPath(componentPath: string, candidate: string): string {
  if (path.isAbsolute(candidate)) return candidate;
  if (componentPath.startsWith("/")) {
    return path.posix.resolve(path.posix.dirname(componentPath), candidate);
  }
  return path.resolve(path.dirname(componentPath), candidate);
}
