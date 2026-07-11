import { ConfigSourceSpec } from "../cli";
import { EdgeCommonsError } from "../errors";
import { logger } from "../logging";
import { sanitize } from "./template";
import { ConfigSource, ConfigWatch } from "./source";
import { cloneJson, deepMerge, isJsonObject, JsonObject } from "./merge";

export interface LayeredConfigCoordinatorOptions {
  source: ConfigSource;
  sourceSpec: ConfigSourceSpec;
  componentName: string;
}

export interface ResolvedConfigLayer {
  id: string;
  kind: "scope" | "component";
  scope?: JsonObject;
  component?: string;
  config: JsonObject;
}

export interface LineageBundle {
  lineageVersion: 1;
  catalogVersion: string;
  component: string;
  layers: ResolvedConfigLayer[];
}

interface LayerCandidate {
  effective: JsonObject;
}

export class LayeredConfigCoordinator {
  private latestEffective?: JsonObject;
  private sourceWatch?: ConfigWatch;
  // Serialize source updates so a slow asynchronous validator cannot commit an older generation
  // after a later candidate. Rejections are contained in the individual result and never poison
  // the following update.
  private updateTail: Promise<void> = Promise.resolve();

  constructor(private readonly opts: LayeredConfigCoordinatorOptions) {}

  async loadEffective(): Promise<JsonObject> {
    const raw = await this.opts.source.load();
    const candidate = this.candidateFromSource(raw);
    this.commit(candidate);
    return cloneJson(candidate.effective);
  }

  async reloadFromProvider(
    applyEffective: (effective: JsonObject) => boolean | Promise<boolean>,
  ): Promise<boolean> {
    let raw: unknown;
    try {
      raw = await this.opts.source.load();
    } catch (e) {
      logger.warn(
        `reload-config: re-fetch from the '${this.opts.source.sourceName()}' source failed: ${String(e)}`,
      );
      return false;
    }
    return this.applySourceUpdate(raw, applyEffective);
  }

  async watch(
    applyEffective: (effective: JsonObject) => boolean | Promise<boolean>,
  ): Promise<ConfigWatch | undefined> {
    this.sourceWatch = await this.opts.source.watch((raw) => {
      void this.applySourceUpdate(raw, applyEffective);
    });
    return this.sourceWatch;
  }

  private applySourceUpdate(
    raw: unknown,
    applyEffective: (effective: JsonObject) => boolean | Promise<boolean>,
  ): Promise<boolean> {
    const run = async (): Promise<boolean> => this.applySourceUpdateNow(raw, applyEffective);
    const result = this.updateTail.then(run, run);
    this.updateTail = result.then(
      () => undefined,
      () => undefined,
    );
    return result;
  }

  private async applySourceUpdateNow(
    raw: unknown,
    applyEffective: (effective: JsonObject) => boolean | Promise<boolean>,
  ): Promise<boolean> {
    let candidate: LayerCandidate;
    try {
      candidate = this.candidateFromSource(raw);
    } catch (e) {
      logger.warn(`reloaded config rejected; keeping previous: ${String(e)}`);
      return false;
    }
    if (!(await applyEffective(candidate.effective))) {
      return false;
    }
    this.commit(candidate);
    return true;
  }

  private candidateFromSource(raw: unknown): LayerCandidate {
    if (this.opts.sourceSpec.kind === "CONFIG_COMPONENT") {
      const bundle = parseConfigComponentPayload(raw, requestedComponentToken(this.opts.componentName));
      const merged = mergeLineageLayers(bundle.layers);
      return { effective: merged.effective };
    }
    return { effective: requireObject(raw, "config document") };
  }

  private commit(candidate: LayerCandidate): void {
    this.latestEffective = cloneJson(candidate.effective);
  }

  latestSnapshot(): JsonObject | undefined {
    return this.latestEffective === undefined ? undefined : cloneJson(this.latestEffective);
  }
}

export function parseConfigComponentPayload(raw: unknown, requestComponent?: string): LineageBundle {
  const body = requireObject(raw, "CONFIG_COMPONENT payload");
  rejectStructuredError(body);
  const lineageVersion = body.lineageVersion;
  const catalogVersion = body.catalogVersion;
  const component = body.component;
  const rawLayers = body.layers;

  if (
    lineageVersion !== 1 ||
    typeof catalogVersion !== "string" ||
    typeof component !== "string" ||
    !Array.isArray(rawLayers) ||
    rawLayers.length === 0
  ) {
    throw lineageInvalid("payload must contain lineageVersion:1, catalogVersion, component, and non-empty layers[]");
  }

  if (requestComponent !== undefined && component !== requestComponent) {
    throw lineageInvalid(`bundle component '${component}' does not match requested component '${requestComponent}'`);
  }

  const layers = rawLayers.map((layer, index) =>
    parseLayer(layer, index, rawLayers.length, component),
  );
  validateScopeOwnership(layers);
  validateIdentityOwnership(layers);

  return {
    lineageVersion: 1,
    catalogVersion,
    component,
    layers,
  };
}

export function mergeLineageLayers(layers: ResolvedConfigLayer[]) {
  return deepMerge(layers.map((layer) => layer.config));
}

function parseLayer(
  raw: unknown,
  index: number,
  layerCount: number,
  bundleComponent: string,
): ResolvedConfigLayer {
  const layer = requireObject(raw, `CONFIG_COMPONENT layer ${index}`);
  if (typeof layer.id !== "string" || layer.id.trim() === "") {
    throw lineageInvalid(`layer ${index} id must be a non-empty string`);
  }
  if (layer.kind !== "scope" && layer.kind !== "component") {
    throw lineageInvalid(`layer ${index} kind must be 'scope' or 'component'`);
  }
  if (layer.kind === "component") {
    if (index !== layerCount - 1) {
      throw lineageInvalid("component layer must be final");
    }
    if (layer.component !== bundleComponent) {
      throw lineageInvalid(`component layer '${layer.id}' does not match bundle component '${bundleComponent}'`);
    }
  } else if (index === layerCount - 1) {
    throw lineageInvalid("final layer must be kind 'component'");
  } else if (!isJsonObject(layer.scope)) {
    throw lineageInvalid(`scope layer '${layer.id}' must contain object scope`);
  }
  if (!isJsonObject(layer.config)) {
    throw lineageInvalid(`layer ${index} config must be an object`);
  }
  if (layer.scope !== undefined) {
    requireStringMap(layer.scope, `layer ${index} scope`);
  }
  if (layer.component !== undefined && typeof layer.component !== "string") {
    throw lineageInvalid(`layer ${index} component must be a string when present`);
  }
  const scope = layer.scope === undefined ? undefined : requireStringMap(layer.scope, `layer ${index} scope`);
  return {
    id: layer.id,
    kind: layer.kind,
    scope: scope === undefined ? undefined : cloneJson(scope),
    component: layer.component,
    config: cloneJson(layer.config),
  };
}

function validateScopeOwnership(layers: ResolvedConfigLayer[]): void {
  const owned: Record<string, string> = {};
  for (const layer of layers) {
    if (layer.scope === undefined) continue;
    for (const [key, value] of Object.entries(layer.scope)) {
      if (typeof value !== "string") {
        throw lineageInvalid(`scope value for '${key}' must be a string`);
      }
      const previous = owned[key];
      if (previous !== undefined && previous !== value) {
        throw EdgeCommonsError.config(
          `LINEAGE_SCOPE_CONFLICT: scope '${key}' changed from '${previous}' to '${value}' at layer '${layer.id}'`,
        );
      }
      owned[key] = value;
    }
  }
}

function validateIdentityOwnership(layers: ResolvedConfigLayer[]): void {
  const owned: Record<string, unknown> = {};
  for (const layer of layers) {
    const identity = layer.config.identity;
    if (identity === undefined) continue;
    if (!isJsonObject(identity)) {
      continue;
    }
    for (const [key, value] of Object.entries(identity)) {
      if (Object.prototype.hasOwnProperty.call(owned, key) && !jsonEqual(owned[key], value)) {
        throw EdgeCommonsError.config(
          `LINEAGE_IDENTITY_CONFLICT: identity '${key}' changed at layer '${layer.id}'`,
        );
      }
      owned[key] = cloneJsonValue(value);
    }
  }
}

function jsonEqual(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function cloneJsonValue<T>(value: T): T {
  if (value === undefined) return value;
  return JSON.parse(JSON.stringify(value)) as T;
}

function rejectStructuredError(body: JsonObject): void {
  if (body.ok === false && isJsonObject(body.error)) {
    const code = typeof body.error.code === "string" ? body.error.code : "CONFIG_COMPONENT_ERROR";
    const message =
      typeof body.error.message === "string" ? body.error.message : "CONFIG_COMPONENT server error";
    throw EdgeCommonsError.config(`${code}: ${message}`);
  }
}

function requireStringMap(value: unknown, label: string): JsonObject {
  if (!isJsonObject(value)) {
    throw lineageInvalid(`${label} must be an object`);
  }
  for (const [key, item] of Object.entries(value)) {
    if (typeof item !== "string") {
      throw lineageInvalid(`${label}.${key} must be a string`);
    }
  }
  return value;
}

function requireObject(value: unknown, label: string): JsonObject {
  if (!isJsonObject(value)) {
    throw EdgeCommonsError.config(`${label} must be a JSON object`);
  }
  return cloneJson(value);
}

function lineageInvalid(message: string): EdgeCommonsError {
  return EdgeCommonsError.config(`LINEAGE_BUNDLE_INVALID: ${message}`);
}

function requestedComponentToken(componentName: string): string {
  const shortName = componentName.includes(".")
    ? componentName.slice(componentName.lastIndexOf(".") + 1)
    : componentName;
  return sanitize(shortName);
}
