# How-to guides

Task recipes. Each one assumes you already have a component project; if you do not, start with the
[tutorial](tutorial.md).

## Move a component to a new library version

`upgrade` changes the **edgecommons library** version your component depends on. It is a different
verb from `version`, which changes the component's *own* version — conflating them is the mistake
this split exists to prevent.

```bash
edgecommons component upgrade --to 0.3.0 --dry-run   # see the rewrite first
edgecommons component upgrade --to 0.3.0
```

Rust and Python projects may pin the library by git revision instead of a released version. To move
that pin:

```bash
edgecommons component upgrade --to-rev 9f2c1ab
```

`--to` and `--to-rev` are mutually exclusive: one moves you to a release tag, the other to a raw
revision. Always run `--dry-run` first — it prints every manifest it would touch and changes nothing.

If the project declares no dependency manifest the tool can operate on, you get `EC4004` rather than
a silent no-op.

## Set the component's own version

```bash
edgecommons component version --to 1.2.0 --dry-run
edgecommons component version --to 1.2.0
```

This updates the supported version fields in `Cargo.toml`, `package.json`, `pom.xml`, and
`gdk-config.json`. It does not currently update `recipe.yaml` or a Python project's own version in
`pyproject.toml`; review those separately. The stated version is authoritative; the tool validates
the numeric dotted string rather than inferring one from commit history.

## Validate before every commit

```bash
edgecommons component validate --platform GREENGRASS
```

Run it for each platform you actually target — rules differ per platform, and omitting `--platform`
silently skips the platform-dependent ones:

```bash
for p in GREENGRASS HOST KUBERNETES; do
  edgecommons component validate --platform "$p" || exit 1
done
```

To check one specific file rather than every config the project ships:

```bash
edgecommons component validate --config config/production.json --platform HOST
```

## Package and publish for Greengrass

```bash
edgecommons doctor --platforms GREENGRASS        # gdk present and new enough?
edgecommons component package --platforms GREENGRASS
edgecommons component package --platforms GREENGRASS --publish
```

`--publish` runs `gdk component publish`, which needs AWS credentials in the environment. Two errors
you will hit if the scaffold has not been finished:

- `EC3007` — `gdk-config.json` still has the placeholder publish bucket, so it cannot publish.
- `EC4005` — a Greengrass scaffold with no artifact bucket at all.

Both mean the same thing: decide where artifacts live before trying to ship them.

## Cut a release descriptor

```bash
edgecommons component release --out release.json
```

Build the artifacts before running this command. It hashes already-staged Greengrass files and
writes a machine-readable release descriptor; it does not run a build. Container image coordinates
and supply-chain evidence remain for the release workflow to fill. It **never tags, uploads, or
publishes**. Set a concrete component version first if GDK still declares `NEXT_PATCH`.

## Find a component in the ecosystem

```bash
edgecommons registry list
edgecommons registry list --category adapter --language RUST
edgecommons registry list --category tool
edgecommons registry show opcua-adapter
edgecommons registry versions opcua-adapter
```

Use a local copy of a different catalog with `--source`, or set
`EDGECOMMONS_REGISTRY_URL` once:

```bash
export EDGECOMMONS_REGISTRY_URL=./my-registry/components.json
edgecommons registry list
```

The default uses authenticated `gh`; HTTP(S) source URLs are unsupported. `registry versions`
currently reports that release-index lookup is unavailable rather than listing versions.

## Render a deployment

A deployment definition describes a site: its hierarchy, its nodes, and which components run on each.
The renderer compiles it into what the target platform actually consumes.

```bash
edgecommons deployment validate site.yaml
edgecommons deployment plan   site.yaml --env prod --target HOST
edgecommons deployment render site.yaml --env prod --target HOST
```

`validate` runs four stages: the definition's own schema, the semantic rules, **every rendered
effective config** against the strict runtime schema — so a config that only breaks once the
hierarchy is merged is caught before anything is written — and finally the compatibility guard
against the lock (below).

`plan` prints the normalized target plan: per node, per component, the desired configuration and whether a config update
**restarts the component**. Restart impact is derived from each component's config source, never
assumed — a watched file or a catalog push is picked up live, an environment change is not.

`render` writes the artifacts under `render/<target>/` and commits nothing.

For Greengrass, the unit is the **thing**, not the thing group: a definition's nodes map one-to-one
onto deployment documents, so failure is per node.

```bash
edgecommons deployment render site.yaml --env prod --target GREENGRASS
```

## Lock the versions a definition pins

A definition pins component versions. `lock` resolves those pins and writes what they resolved to,
so everything downstream reads files that are already in Git:

```bash
edgecommons deployment lock site.yaml
git add site.lock && git commit -m "lock component versions"
```

This uses authenticated `gh` by default. A local catalog file lets you lock without a network request
or `gh` credentials:

```bash
edgecommons deployment lock site.yaml --source ../registry/components.json
```

The lock carries each pinned version's artifact digest, the config schema that version publishes, and
its Greengrass component name. Once it is committed, `validate`, `render`, and `plan` need no network
at all, and a Greengrass render no longer needs `artifact.greengrassName` in the definition.

Re-run it whenever you change a pin. What it cannot resolve it records **with the reason** and reports
as a warning. The current registry adapter resolves component names but leaves per-version digests
and schemas unverified; the lock and validation diagnostics expose that limitation.

## Promote a release

Config and artifacts are two independently versioned streams, and you generate release files for
one at a time. This command currently requires exactly one profile and one environment:

```bash
edgecommons deployment release site.yaml --stream config
edgecommons deployment release site.yaml --stream artifact
```

The release lock **correlates** the two without fusing them: it records what was in effect together,
and either stream can roll back alone. A config change ships without reshipping the binary, and the
reverse.

Output is written locally under `releases/<tag>/`. Publication and application are separate runner
work; this command does not change devices.

## Use it in CI

Two flags make the tool behave in an automated job: `--json` for structured output and `--yes` so a
missing input fails instead of waiting for a prompt.

```yaml
- run: edgecommons component validate --platform GREENGRASS --json --yes
- run: edgecommons deployment validate site.yaml --json --yes
```

Branch on the exit code, not on the text: `0` clean, `1` findings, `2` you invoked it wrong, `3` a
required tool is missing, `5` the verb is not built in this binary. See
[exit codes](reference/exit-codes.md).

## Add shell completion

```bash
edgecommons completions bash > /etc/bash_completion.d/edgecommons
edgecommons completions zsh  > "${fpath[1]}/_edgecommons"
edgecommons completions fish > ~/.config/fish/completions/edgecommons.fish
edgecommons completions powershell | Out-String | Invoke-Expression
```

`elvish` is also supported.

## Diagnose a failing environment

```bash
edgecommons doctor
edgecommons doctor --platforms GREENGRASS --language JAVA
```

`doctor` never installs anything. It reports what is missing (`EC0001`) or too old (`EC0002`) and
leaves the fixing to you and your package manager.
