# EdgeCommons hierarchical config conformance vectors

These vectors define the public hierarchical config contract used by
ConfigComponent and by all four language libraries.

The canonical fixture hierarchy is:

```text
enterprise -> site -> zone -> line -> device
```

Catalogs define the scope layers through `line`. The `device` value is supplied
by the runtime thing identity, not by the catalog. The component entry is the
leaf of the resolved lineage but is not itself a hierarchy level.

All clients consume the same wire shape:

```json
{
  "lineageVersion": 1,
  "catalogVersion": "enterprise-site-zone-line-v1",
  "component": "opcua-adapter",
  "layers": [
    { "id": "enterprise/acme", "kind": "scope", "config": {} },
    { "id": "component/opcua-adapter", "kind": "component", "config": {} }
  ]
}
```

Language tests should parse `layers[]`, validate lineage ownership, merge only
`layers[].config`, validate the final effective config, and retain the previous
effective snapshot on reload failure.
