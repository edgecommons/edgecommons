# edgecommons component registry

The machine-readable catalog of components in the **edgecommons** ecosystem — protocol adapters,
edge processors, and northbound sinks built on the [`ggcommons`](https://github.com/edgecommons/ggcommons)
library.

- **`components.json`** — the catalog (source of truth for "what components exist").
- **`registry.schema.json`** — JSON Schema the catalog is validated against in CI.

## Consumers

- **CLI:** `ggcommons list-components` reads this catalog (defaults to the `main` branch raw URL;
  override with `--source` or `$GGCOMMONS_REGISTRY_URL`).
- **Docs site:** renders a "Components" page from `components.json`.

## Adding or updating a component

Open a pull request editing `components.json` — see [`CONTRIBUTING.md`](CONTRIBUTING.md). CI validates
the file against the schema before merge.

This repository is intentionally **public** so the catalog is readable without authentication, even
while individual component repositories may be private.
