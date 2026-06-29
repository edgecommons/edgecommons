# edgecommons cutover runbook (Phase 0–1)

Operational steps to (0) create the org + a self-hosted CI runner and (1) move the `ggcommons`
library into the org and repoint its package coordinates. See `docs/ECOSYSTEM.md` for the overall
design. **Do these in order; the repointing (Phase 1) only fully "activates" after the transfer +
republish.**

---

## Phase 0a — Create the org

1. https://github.com/account/organizations/new → **Free** plan → name **`edgecommons`**.
   (Org creation is web-only — not in `gh`/the API.)
2. Org **Settings → Member privileges**: base permission = your choice; default new repo = **private**.
3. Org **Settings → Actions → General**: enable Actions; under *Fork pull request workflows* keep
   approvals **required** (matters once you add the self-hosted runner — see security note below).

---

## Phase 0b — Self-hosted CI runner on lab-5950x (detailed)

A self-hosted runner gives the org **unlimited** Actions minutes (a Free org is capped at 2,000
private min/mo). lab-5950x already runs your validation, so it's the natural host.

> ⚠️ **Security: never let a self-hosted runner build a _public_ repo.** Anyone can open a PR that
> then executes arbitrary code on lab-5950x (which also runs your Greengrass nucleus + k3s). Keep
> repos using this runner **private**, or use GitHub-hosted runners (free + unlimited) for any public
> repo. Keep *fork PR approvals required* (Phase 0a step 3).

### 1. Get the registration token + commands (org-level)

In the org: **Settings → Actions → Runners → New runner → New self-hosted runner → Linux / x64**.
GitHub shows the exact, current commands with a **short-lived token** (≈1 hour TTL) and the current
runner version. Run them on lab-5950x. The shape is:

```bash
ssh marc@192.168.1.229

# Dedicated, unprivileged location (do NOT run the runner as root).
mkdir -p ~/actions-runner && cd ~/actions-runner

# Download the version the UI shows (vX.Y.Z), then validate + extract:
curl -o actions-runner-linux-x64.tar.gz -L \
  https://github.com/actions/runner/releases/download/vX.Y.Z/actions-runner-linux-x64-X.Y.Z.tar.gz
tar xzf ./actions-runner-linux-x64.tar.gz

# Register against the ORG (note: /edgecommons, not a repo URL), with useful labels:
./config.sh --url https://github.com/edgecommons \
  --token <REGISTRATION_TOKEN_FROM_UI> \
  --name lab-5950x \
  --labels self-hosted,linux,x64,lab-5950x \
  --unattended
```

Outbound HTTPS to github.com is all that's needed — **no inbound ports**. The runner long-polls.

### 2. Install as a service (survives reboot)

```bash
cd ~/actions-runner
sudo ./svc.sh install marc      # run the service as 'marc' (or a dedicated svc user)
sudo ./svc.sh start
sudo ./svc.sh status            # confirm "active (running)"
```

The runner now appears under **Settings → Actions → Runners** as `lab-5950x` (Idle).

### 3. Scope it to specific repos (recommended)

Org **Settings → Actions → Runner groups** → create a group, add the runner, and restrict it to
**selected repositories**. This prevents an unexpected new repo from being able to run jobs on
lab-5950x.

### 4. Isolate it from Greengrass / k3s

CI builds (`mvn verify`, `cargo build`, `npm ci`) are heavy and share the box with the nucleus + k3s:

- Run the service as a non-privileged user; optionally cap it with a systemd drop-in:
  ```ini
  # sudo systemctl edit actions.runner.edgecommons.lab-5950x.service
  [Service]
  CPUQuota=400%        # ~4 cores of the 5950x's 16
  MemoryMax=8G
  Nice=10
  ```
- Avoid running CI during on-device Greengrass validation windows.
- Alternative for stronger isolation: run jobs in containers (`container:` in the job) or move to
  **actions-runner-controller (ARC)** as a k3s deployment — heavier to set up, but ephemeral pods per
  job. Start with the single service runner; graduate to ARC only if contention bites.

### 5. Toolchains

The runner host needs the build toolchains (or use container jobs). lab-5950x already has **JDK 25**;
add what's missing for the other languages you'll build there:

```bash
# examples — match your component CI needs
sudo apt-get install -y maven                      # Java builds
# Python: pyenv or system 3.12 ; Node: nvm/nodesource 20 ; Rust: rustup
```

### 6. Use it from workflows

In a component repo, target the runner instead of `ubuntu-latest`:

```yaml
jobs:
  ci:
    runs-on: [self-hosted, lab-5950x]
```

The staged reusable workflow (`ecosystem/staging/org-dotgithub/.github/workflows/component-ci.yml`)
defaults to `ubuntu-latest`; add a `runs-on` input (or a variant) if you want components to opt into
the self-hosted runner.

### Removing a runner

```bash
cd ~/actions-runner
sudo ./svc.sh stop && sudo ./svc.sh uninstall
./config.sh remove --token <REMOVE_TOKEN_FROM_UI>
```

---

## Phase 1a — Transfer the library

1. `github.com/mbreissi/ggcommons` → **Settings → Danger Zone → Transfer** → new owner `edgecommons`.
   GitHub **redirects the old path indefinitely**, so existing clones/CI/package refs keep working.
2. Update your local remote:
   ```bash
   git remote set-url origin git@github.com:edgecommons/ggcommons.git
   ```
3. **Re-authorize the Cloudflare Workers Builds GitHub App** on the `edgecommons` org (the docs site
   auto-deploys from this repo) and confirm a push still triggers a docs deploy.

---

## Phase 1b — Repoint package coordinates

### What changes vs. what must NOT

| Reference | Action | Why |
|-----------|--------|-----|
| `github.com/mbreissi/…` (git URLs, **Maven** `maven.pkg.github.com/mbreissi/…`, npm.pkg URLs) | → `edgecommons` | Repo owner moved. |
| `ghcr.io/mbreissi/…` (component image namespace) | → `edgecommons` | GHCR namespace = owner. |
| npm scope **`@mbreissi/ggcommons`** (+ `@mbreissi:registry`, `scope: "@mbreissi"`) | → `@edgecommons` | **GitHub Packages npm forces scope = owner.** This is the biggest surface (all TS imports/templates/docs). |
| Public-npm addon **`@mbreissi/ggstreamlog-node`** | optional | Published to *public* npm (registry.npmjs.org), so its scope is independent of the GitHub owner. Rename for consistency (needs you to own the `@edgecommons` npm org) or keep on `@mbreissi`. |
| **`com.mbreissi`** (Java groupId / package, ~hundreds of refs) | **keep** | Maven coordinates are independent of the GH Packages owner — only the `<distributionManagement>`/repository **URL** changes (covered by the `github.com/mbreissi/` rule). |
| **`docs.ggcommons.mbreissi.com`** (docs domain) | **keep** | A DNS subdomain, unrelated to the GitHub owner. |

> **Do NOT do a blanket `mbreissi` → `edgecommons` replace** — it would break the Java groupId and the
> docs domain. Use the targeted rules below (none of them match `com.mbreissi` or `…mbreissi.com`).

### The three safe rules

1. `github.com/mbreissi/`  → `github.com/edgecommons/`  (covers git, `maven.pkg.github.com/mbreissi/`, `npm.pkg.github.com/mbreissi/`)
2. `ghcr.io/mbreissi/`     → `ghcr.io/edgecommons/`
3. `@mbreissi`             → `@edgecommons`  (npm package names, `scope:` params, `.npmrc` registry maps)

### Run it (dry-run first)

```powershell
# from the repo root (PowerShell, the primary shell here)
pwsh ecosystem/repoint-to-edgecommons.ps1                 # DRY RUN — lists every file that would change
pwsh ecosystem/repoint-to-edgecommons.ps1 -Apply         # write the changes
pwsh ecosystem/repoint-to-edgecommons.ps1 -Apply -KeepAddonScope   # ...but leave @mbreissi/ggstreamlog-node alone
```
(Bash equivalent: `ecosystem/repoint-to-edgecommons.sh` / `--apply`.)

The script skips `.git`, `node_modules`, `target`, `build`, `dist`, the whole `ecosystem/` dir, and
`docs/ECOSYSTEM.md` (those last two document the *before→after* mapping and must stay literal).

### After applying

```bash
npm install                  # regenerate package-lock.json cleanly under the new scope
# rebuild + test all four:
mvn -q -f libs/java/pom.xml verify
( cd libs/ts && npm run build && npm test )
( cd libs/python && python -m pytest -q )
( cd libs/rust && cargo test )
( cd cli && python -m pytest -q )           # CLI tests assert the scaffolded dep strings
```

### Then publish under the org (activation)

Until packages are republished under `edgecommons`, *registry* installs of the new coordinates 404
(local `file:`/path deps keep working). Cut a release tag per artifact:

```bash
git tag java-lib/vX.Y.Z && git push origin java-lib/vX.Y.Z   # → GH Packages Maven (edgecommons)
git tag ts-lib/vX.Y.Z   && git push origin ts-lib/vX.Y.Z     # → GH Packages npm @edgecommons/ggcommons
# python-lib / rust-lib: the tag IS the release (git-dep consumers); push the tags likewise.
```

`release.yml` authenticates with the built-in `GITHUB_TOKEN`/`github.actor`, so it adapts to the new
owner automatically once `scope:`/coordinates are repointed.

### Cutover checklist

- [ ] Repoint script applied; `npm install` regenerated the lock.
- [ ] All four libs + CLI build and test green.
- [ ] `java-lib` / `ts-lib` republished to GH Packages under `edgecommons`; a clean consumer resolves them.
- [ ] `python-lib` / `rust-lib` tags pushed; a `pip git+https@<tag>` / cargo git-dep resolves.
- [ ] Docs site rebuilt (Cloudflare) with the new coordinates; install page verified.
- [ ] Decide the public-npm addon scope (`-KeepAddonScope` or rename + own `@edgecommons` on npm).
