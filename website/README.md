# EdgeCommons documentation website

Astro Starlight builds the shared library guides and ecosystem component/tool documentation.
The configured public site is `https://docs.edgecommons.mbreissi.com`.

## Authoring sources

- `src/content/docs/{start,guides,reference,deploy}` contains hand-authored shared pages.
- Component repositories own their `docs/`; `core/cli/docs/` owns CLI documentation.
- `scripts/sync-component-docs.mjs` reads `registry/components.json` and generates the gitignored
  `src/content/docs/components/` and `tools/` trees. Edit owning sources, then regenerate.
- Brand tokens originate in `brand/tokens/edgecommons.tokens.json`. Run the brand repository's
  generation/sync commands to refresh vendored CSS and assets; do not edit generated token CSS.

## Build and verify

Run `npm install`, then `npm run build` here. The build syncs catalog docs, builds Astro, and checks
internal links and anchors. `npm run dev` previews on port 4321; `npm run preview` serves `dist/`.

For local workspace changes, set `REGISTRY_JSON` to the local registry JSON and `COMPONENT_DOCS_MAP`
to a JSON object mapping every catalog id to its local `docs/` directory. Include the CLI id mapped
to `core/cli/docs/`. Prefer absolute paths. Inspect the sync log and generated page inventory: missing
sources can produce warnings or fallbacks without failing the build. Build success alone does not
prove catalog completeness.

Use current files and Git history for source review; CodeGraph and Graphify are disabled.
