/**
 * Metrics — model: {@link Measure} / {@link Metric} value types and the fluent
 * {@link MetricBuilder}. Mirrors the Java/Python/Rust metric model.
 *
 * The builder injects the standard dimensions `coreName` (thing name), `category`
 * (metric name), and `component` (component name) when known. `Measure` storage
 * resolution is coerced to CloudWatch's allowed values: `1` (high resolution) for
 * anything `< 60`, else `60`. Dimensions/measures keep insertion order via `Map`
 * for stable EMF output.
 */

/** One measurement within a {@link Metric}. */
export class Measure {
  readonly name: string;
  readonly unit: string;
  readonly storageResolution: number;

  constructor(name: string, unit: string, storageResolution: number) {
    this.name = name;
    this.unit = unit;
    this.storageResolution = storageResolution < 60 ? 1 : 60;
  }
}

/** A metric definition: name, namespace, measures, and string dimensions. */
export class Metric {
  readonly name: string;
  readonly namespace?: string;
  readonly measures: Map<string, Measure>;
  readonly dimensions: Map<string, string>;

  constructor(
    name: string,
    namespace: string | undefined,
    measures: Map<string, Measure>,
    dimensions: Map<string, string>,
  ) {
    this.name = name;
    this.namespace = namespace;
    this.measures = measures;
    this.dimensions = dimensions;
  }

  getName(): string {
    return this.name;
  }
  getNamespace(): string | undefined {
    return this.namespace;
  }
  getMeasures(): Map<string, Measure> {
    return this.measures;
  }
  getMeasure(name: string): Measure | undefined {
    return this.measures.get(name);
  }
  getDimensions(): Map<string, string> {
    return this.dimensions;
  }
}

/** Fluent builder for {@link Metric} (the supported construction path). */
export class MetricBuilder {
  private namespace_?: string;
  private thingName_?: string;
  private componentName_?: string;
  private readonly measures = new Map<string, Measure>();
  private readonly dimensions = new Map<string, string>();

  private constructor(private readonly name: string) {}

  /** Start building a metric with the given name. */
  static create(name: string): MetricBuilder {
    return new MetricBuilder(name);
  }

  /** Set the metric namespace. */
  withNamespace(namespace: string): this {
    this.namespace_ = namespace;
    return this;
  }

  /** Set the thing name (becomes the `coreName` dimension). */
  withThingName(thingName: string): this {
    this.thingName_ = thingName;
    return this;
  }

  /** Set the component name (becomes the `component` dimension). */
  withComponentName(componentName: string): this {
    this.componentName_ = componentName;
    return this;
  }

  /**
   * Populate thing name, component name, and namespace from a config snapshot.
   * Typed structurally to avoid a config import.
   */
  withConfig(config: {
    thingName: string;
    componentName: string;
    parsed: { metricEmission: { namespace(): string } };
  }): this {
    this.thingName_ = config.thingName;
    this.componentName_ = config.componentName;
    if (this.namespace_ === undefined) {
      this.namespace_ = config.parsed.metricEmission.namespace();
    }
    return this;
  }

  /** Add a measure with the given unit and storage resolution. */
  addMeasure(name: string, unit: string, storageResolution: number): this {
    const measure = new Measure(name, unit, storageResolution);
    this.measures.set(measure.name, measure);
    return this;
  }

  /** Add a custom dimension. */
  addDimension(key: string, value: string): this {
    this.dimensions.set(key, value);
    return this;
  }

  /** Build the metric, injecting the standard `category`/`coreName`/`component` dimensions. */
  build(): Metric {
    const dims = new Map(this.dimensions);
    dims.set("category", this.name);
    if (this.thingName_ !== undefined) dims.set("coreName", this.thingName_);
    if (this.componentName_ !== undefined) dims.set("component", this.componentName_);
    return new Metric(this.name, this.namespace_, new Map(this.measures), dims);
  }
}
