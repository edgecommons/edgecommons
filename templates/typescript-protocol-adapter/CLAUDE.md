# <<COMPONENTNAME>> — component notes

EdgeCommons **protocol adapter** component (TypeScript). Full name `<<COMPONENTFULLNAME>>`.
The shape, seam, config, and validation expectations live in `AGENTS.md` and are shared with every
agent tool. It is imported here in full:

@AGENTS.md

## Local dev

- **`--dep-source local`** (the default): `package.json` depends on `@edgecommons/edgecommons` via
  a `file:` path into the sibling checkout. Build the sibling first (`npm run build` in
  `core/libs/ts`) before `npm install` here — a `file:` dependency on a TypeScript package needs its
  `dist/` present.
- **`--dep-source registry`**: depends on the published `@edgecommons/edgecommons` npm package
  instead; no sibling build step, and the `.npmrc` this scaffold ships points at the package
  registry. Regenerate with this flag once you're pushing a real component to its own repo.
