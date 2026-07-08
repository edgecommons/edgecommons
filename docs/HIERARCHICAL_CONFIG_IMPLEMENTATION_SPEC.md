# Hierarchical Configuration - Binding Implementation Specification

This specification defines the current EdgeCommons hierarchical configuration contract across Java,
Python, Rust, TypeScript, and `com.mbreissi.edgecommons.ConfigComponent`.

Java remains canonical for core library behavior. Observable behavior, wire bodies, validation
errors, and reload semantics must remain equivalent across all four SDKs, with language-idiomatic
API shapes where the SDKs already differ.

## 1. Scope

The implementation replaces the two-layer shared/component model with a true lineage model:

- Direct config providers supply one effective document.
- `CONFIG_COMPONENT` supplies a lineage bundle with an arbitrary number of ordered layers.
- Clients merge `layers[].config`, validate the merged effective document, and expose only that
  effective config through existing APIs.
- ConfigComponent owns catalog parsing, lineage assembly, `get-configuration`, `update-catalog`,
  and `set-config` fanout.

The hierarchy can contain as many user-defined levels as needed, provided the final level is
`device`. A common test hierarchy is:

```text
enterprise -> site -> zone -> line -> device
```

The runtime device is not represented as a catalog node. Its value comes from `-t/--thing`, the
Kubernetes Downward API, or Greengrass/AWS IoT environment resolution.

## 2. Removed Surface

These are not part of the public authoring or runtime model:

- `--no-shared-config`.
- `sharedConfig`.
- `extends`.
- `EDGECOMMONS_SHARED_CONFIG`.
- `EDGECOMMONS_SHARED_COMPONENT`.
- Provider-family base resolvers.
- `CONFIG_COMPONENT` legacy component-only replies.
- `CONFIG_COMPONENT` `{base, component}` replies.
- ConfigComponent catalogs with a top-level `base`.

SDKs may reject removed flags with explicit guidance. Direct providers do not treat removed config
fields as controls; they simply validate the final effective document like any other top-level key.

## 3. ConfigComponent Catalog

Catalog JSON:

```json
{
  "schemaVersion": 1,
  "version": "enterprise-site-zone-line-v1",
  "provenance": { "source": "file", "uri": "/etc/edgecommons/catalog.json" },
  "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
  "nodes": {},
  "components": {}
}
```

Required behavior:

- Catalog must be a JSON object.
- `schemaVersion` must be `1`.
- `version` must be non-empty for message-delivered catalogs.
- Source-loaded catalogs may derive `version` from content hash when absent.
- `provenance`, when present, must be an object.
- `hierarchy.levels` must be a non-empty array of unique non-empty strings.
- `hierarchy.levels.last` must be `device`.
- `nodes` and `components` must be objects.
- Top-level `base` is invalid.

Node entry:

```json
{
  "parent": "site/dallas",
  "scope": { "enterprise": "acme", "site": "dallas", "zone": "packaging" },
  "config": { "identity": { "zone": "packaging" } }
}
```

Component entry:

```json
{
  "parent": "line/line-7",
  "config": { "component": { "token": "opcua-adapter" } }
}
```

Validation:

- Node ids are `<level>/<value>`.
- Node id level must exist in `hierarchy.levels` and must not be `device`.
- `scope` must be a non-empty object and must not contain `device`.
- `scope` must include the node id's own `<level>:<value>` claim.
- `config` must be an object.
- `parent`, when present, must reference an existing node.
- Parent chains must be acyclic.
- Parent chains must not exceed 64 nodes.
- Child scope values must not conflict with ancestor scope values.
- A layer's `identity` values must not conflict with identity values already owned by ancestors.
- Component keys must equal their sanitized form.

Error codes:

- `CATALOG_INVALID`.
- `LINEAGE_CYCLE`.
- `LINEAGE_PARENT_MISSING`.
- `LINEAGE_DEPTH_EXCEEDED`.
- `LINEAGE_SCOPE_CONFLICT`.
- `LINEAGE_IDENTITY_CONFLICT`.

## 4. Lineage Bundle Wire Body

Successful `get-configuration` replies and `set-config` pushes use this body:

```json
{
  "lineageVersion": 1,
  "catalogVersion": "enterprise-site-zone-line-v1",
  "component": "opcua-adapter",
  "provenance": { "source": "file", "uri": "/etc/edgecommons/catalog.json" },
  "layers": [
    {
      "id": "enterprise/acme",
      "kind": "scope",
      "scope": { "enterprise": "acme" },
      "config": { "identity": { "enterprise": "acme" } }
    },
    {
      "id": "component/opcua-adapter",
      "kind": "component",
      "component": "opcua-adapter",
      "config": { "component": { "token": "opcua-adapter" } }
    }
  ]
}
```

Bundle rules:

- `lineageVersion` must be `1`.
- `catalogVersion` must be a non-empty string.
- `component` must match the requested/target component token.
- `layers` must be a non-empty array.
- Each layer must contain a non-empty string `id`.
- Each layer must contain `kind: "scope"` or `kind: "component"`.
- Scope layers precede the component layer and are ordered root-to-leaf.
- Each scope layer must contain object `scope`.
- The final layer must be `kind: "component"` for the target component.
- The component layer must contain `component` equal to the bundle's top-level `component`.
- Each layer must contain object `config`.
- A component layer before the final layer is invalid.
- Old top-level `base` is invalid.

The UNS topics remain unchanged:

- GET: `ecv1/{device}/config/main/cmd/get-configuration`.
- update: `ecv1/{device}/config/main/cmd/update-catalog`.
- push: `ecv1/{device}/{component}/main/cmd/set-config`.

## 5. Client Merge And Reload

Client algorithm:

1. Load source payload.
2. If source is not `CONFIG_COMPONENT`, require it to be an object and treat it as the effective
   document.
3. If source is `CONFIG_COMPONENT`, parse the lineage bundle.
4. Merge `layers[].config` in array order.
5. Validate the merged effective document against `schema/edgecommons-config-schema.json`.
6. Commit component layer, catalog/version metadata, and effective config atomically.
7. Notify listeners only after a valid effective config is committed.

Merge rules:

- Object/object: merge recursively.
- Array: later array replaces earlier value.
- Scalar: later scalar replaces earlier value.
- `null`: later `null` replaces earlier value.

Reload behavior:

- Initial invalid config fails startup.
- Invalid reload keeps the last valid effective config.
- Invalid reload does not notify listeners.
- A structured server error surfaces its embedded error code/message.
- A valid `set-config` bundle replaces the complete lineage snapshot.

## 6. ConfigComponent Bootstrap

The ConfigComponent must not bootstrap from `CONFIG_COMPONENT`.

Allowed bootstrap sources:

- HOST / supervisord: `FILE` or `ENV`.
- Kubernetes: `CONFIGMAP`.
- Greengrass: `GG_CONFIG`.

Bootstrap config lives under the ConfigComponent's own effective config:

```json
{
  "component": {
    "token": "edgecommons-config-component",
    "global": {
      "configComponent": {
        "catalogSource": {
          "type": "file",
          "path": "/etc/edgecommons/catalog.json",
          "watch": true
        },
        "pushOnCatalogReload": true,
        "allowVolatileCatalogUpdates": false
      }
    },
    "instances": []
  }
}
```

`catalogSource.type` supports:

- `file`: `path`, optional `watch`.
- `configmap`: `path` or `mountDir` + `key`, optional `watch`.
- `env`: optional `var`, defaults to `EDGECOMMONS_CONFIG_CATALOG`, snapshot-only.

## 7. Catalog Updates

`update-catalog` is an administrative test/debug interface. It is disabled by default.

Request body:

```json
{
  "version": "enterprise-site-zone-line-v2",
  "catalog": {
    "schemaVersion": 1,
    "version": "enterprise-site-zone-line-v2",
    "hierarchy": { "levels": ["enterprise", "site", "zone", "line", "device"] },
    "nodes": {},
    "components": {}
  }
}
```

Rules:

- `allowVolatileCatalogUpdates` must be `true`.
- `version` must match `catalog.version`.
- The catalog is a complete replacement.
- Valid updates promote to the active in-memory catalog.
- Valid updates do not write to the backing file or ConfigMap.
- Valid updates publish lineage bundles to all components when `pushOnCatalogReload` is true.
- Invalid or disabled updates keep the current active catalog and push nothing.

Ack body:

```json
{
  "ok": true,
  "version": "enterprise-site-zone-line-v2",
  "provenance": {
    "source": "message",
    "interface": "update-catalog",
    "volatile": true
  }
}
```

## 8. Platform Validation

The validation hierarchy for this implementation is:

```text
enterprise -> site -> zone -> line -> device
```

Each line has one runtime device in platform tests.

Required validation:

- Java targeted tests for hierarchical config, ConfigComponent provider, CLI, and removed flag
  rejection.
- Python hierarchical config vector tests.
- Rust core tests, including vector consumption.
- TypeScript hierarchical config tests and build.
- ConfigComponent catalog/source tests.
- Local MQTT interop when wire behavior is in scope.
- Kubernetes hierarchical E2E: `test-infra/k8s/hierarchical-config/run.sh`.
- Greengrass deployed regression on `lab-5950x` using
  `test-infra/interop/gg_hierarchical_config/package.ps1`.

Conformance vectors live in:

```text
hierarchical-config-test-vectors/
```

They pin catalog validation, lineage bundle parsing, merge behavior, and reject-and-keep reload
semantics.
