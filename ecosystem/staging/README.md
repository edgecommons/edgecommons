# Ecosystem staging

This directory holds content **destined for other repositories** in the `edgecommons` GitHub org,
staged here so it can be reviewed in one place and pushed the moment the org exists. None of it is
part of the `ggcommons` library or its build. See `docs/ECOSYSTEM.md` for the full plan.

| Staging path | Target repo | Purpose |
|--------------|-------------|---------|
| `org-dotgithub/` | `edgecommons/.github` | Org profile README (front door), shared community health files, reusable CI workflows. |
| `registry/` | `edgecommons/registry` | Machine-readable component catalog (`components.json` + schema + validation). Public. |

> The nested `org-dotgithub/.github/workflows/*.yml` files are **inert here** — GitHub only runs
> workflows from a repo's root `.github/workflows/`, so they will not trigger on the monorepo.

## Extraction (run once the `edgecommons` org exists)

```bash
# .github repo
gh repo create edgecommons/.github --public -d "edgecommons org defaults + reusable CI"
cd ecosystem/staging/org-dotgithub
git init && git add . && git commit -m "chore: seed edgecommons org defaults"
git branch -M main
git remote add origin git@github.com:edgecommons/.github.git
git push -u origin main

# registry repo
gh repo create edgecommons/registry --public -d "edgecommons component catalog"
cd ../registry
git init && git add . && git commit -m "chore: seed component registry"
git branch -M main
git remote add origin git@github.com:edgecommons/registry.git
git push -u origin main
```

After pushing, the CLI's `list-components` (in `cli/`) will resolve against
`https://raw.githubusercontent.com/edgecommons/registry/main/components.json` automatically.
