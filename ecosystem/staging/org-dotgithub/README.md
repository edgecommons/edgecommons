# edgecommons/.github

Organization defaults for the **edgecommons** ecosystem. GitHub serves these files to every repo in
the org that doesn't provide its own.

- **`profile/README.md`** — the org landing page shown at `github.com/edgecommons`.
- **`CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`** — default community health files.
- **`.github/ISSUE_TEMPLATE/`, `.github/PULL_REQUEST_TEMPLATE.md`** — default issue/PR templates.
- **`.github/workflows/component-ci.yml`** — a **reusable** workflow component repos call via
  `uses: edgecommons/.github/.github/workflows/component-ci.yml@main`.

See [`docs/ECOSYSTEM.md`](https://github.com/edgecommons/ggcommons/blob/main/docs/ECOSYSTEM.md) in
the core repo for the ecosystem design.
