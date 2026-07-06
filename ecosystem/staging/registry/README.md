# edgecommons component registry

The machine-readable catalog of components in the **edgecommons** ecosystem — protocol adapters,
edge processors, and northbound sinks built on the [`edgecommons`](https://github.com/edgecommons/edgecommons)
library.

- **`components.json`** — the catalog (source of truth for "what components exist").
- **`registry.schema.json`** — JSON Schema the catalog is validated against in CI.

## Consumers

- **CLI:** `edgecommons list-components` reads this catalog. This repo is **private**, so by default the
  CLI fetches it with authentication via the GitHub CLI (`gh api .../contents/components.json`).
  Override with `--source <url|path>` or `$EDGECOMMONS_REGISTRY_URL` (e.g. a local clone, or a raw URL
  if this repo is later made public).
- **Docs site:** renders a "Components" page from `components.json`.

## Adding or updating a component

Open a pull request editing `components.json` — see [`CONTRIBUTING.md`](CONTRIBUTING.md). CI validates
the file against the schema before merge.

This repository is **private** (matching the rest of the ecosystem for now); consumers read the
catalog with their GitHub credentials. If you later want tokenless reads, make just this repo public.
