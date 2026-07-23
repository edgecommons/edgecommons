# Tutorial — your first component

By the end of this you will have scaffolded a working EdgeCommons component, understood what the
generator produced, validated it against the canonical config schema, and packaged it for a target
platform. Everything here runs offline.

Allow about twenty minutes. You need the `edgecommons` binary and, to *build* the component you
generate, a toolchain for the language you pick (this tutorial uses Rust).

## 1. Install the CLI

From the library monorepo:

```bash
cargo install --path cli/crates/ec-cli
edgecommons --version
```

## 2. Check your toolchain

Before generating anything, ask the tool what your targets need:

```bash
edgecommons doctor --language RUST --platforms HOST
```

`doctor` reports each external tool it expects, whether it is on your `PATH`, and whether the version
is new enough. A missing tool is `EC0001`; one that is too old is `EC0002`. Nothing is installed for
you — the tool tells you what is wrong and gets out of the way.

If a tool you do not need is missing, that is fine: narrow the check with `--platforms` and
`--language` so you only see what your work actually requires.

## 3. See what you can generate

The binary carries its templates internally. List them:

```bash
edgecommons template list
```

You get a language × kind matrix. The four **kinds** are archetypes, not cosmetic labels — each
generates a different skeleton:

| Kind | What it is |
|---|---|
| `service` | a general component; the default |
| `protocol-adapter` | a southbound adapter that reads a field protocol and publishes signal updates |
| `processor` | consumes messages, transforms them, publishes downstream |
| `sink` | consumes messages and writes them somewhere outside the bus |

Inspect one before you commit to it:

```bash
edgecommons template show rust/protocol-adapter
```

That prints the template's manifest: which platforms it supports, the tokens it substitutes, and
every file it will emit. Nothing is hidden.

## 4. Scaffold the component

```bash
edgecommons component new \
  --name com.example.TankAdapter \
  --language RUST \
  --kind protocol-adapter \
  --description "Reads tank levels and publishes signal updates"
```

The name is the fully-qualified component name. The output directory is derived from it in kebab
form — `tank-adapter` — under the current directory unless you pass `--path`.

Add `--yes` in a script: it turns any missing required input into a usage error instead of an
interactive prompt, so an automated run fails loudly rather than hanging.

## 5. Look at what it generated

```bash
cd tank-adapter
ls
```

The skeleton is a real project, not a stub: source that compiles, a config file that validates,
a `recipe.yaml` and `gdk-config.json` if you target Greengrass, and the packaging metadata for your
language. Build it now if you like — `cargo build` — to prove the loop closes.

## 6. Validate it

This is the step worth internalising, because it is the one you will run constantly:

```bash
edgecommons component validate --platform GREENGRASS
```

Validation runs in three layers, and each catches a different class of mistake:

1. **Schema** — does the config validate against the canonical EdgeCommons schema, and against the
   component's own `config.schema.json` if it publishes one? (`EC1001`, `EC1002`; `EC1003` warns when
   a component publishes no schema, so its own config is unvalidated and the tool says so rather than
   implying coverage it does not have.)
2. **Semantic** — rules a schema cannot express: `--transport IPC` only makes sense on Greengrass
   (`EC2001`), a secret *value* where only a `secret://` reference belongs (`EC2005`), a config
   source that is illegal on the target platform (`EC2009`), a hierarchical-config lineage that
   cycles (`EC2004`).
3. **Artifact lint** — packaging mistakes that only surface at deploy time otherwise: an
   unsubstituted `<<TOKEN>>` left in a recipe (`EC3003`), a `Permissions:` block that
   `CreateComponentVersion` rejects (`EC3002`), `RequiresPrivilege: true` quietly running your
   component as root (`EC3004`).

Passing `--platform` matters. Some rules are only decidable with a platform in hand — a transport or
config source that is illegal on one platform is perfectly legal on another — so without it those
rules are **skipped rather than guessed at**, and you get less coverage than you might assume.

A clean run exits `0`. Findings exit `1`. That distinction is what makes the command useful in CI.

## 7. Package it

```bash
edgecommons component package --platforms HOST
```

This builds the deployable artifacts for the platforms you name. For Greengrass it drives `gdk`;
`--publish` additionally runs `gdk component publish`.

Note what `package` does *not* do: it never tags, uploads, or publishes on its own. That separation is
deliberate — see [Explanation](explanation.md).

## 8. Machine-readable output

Every command speaks JSON:

```bash
edgecommons component validate --platform GREENGRASS --json
```

Use this in CI: the diagnostics come back structured, with their codes, so a job can act on
`EC2005` differently from `EC1003` instead of grepping prose.

## Where next

- Real tasks — upgrading, releasing, deploying: **[How-to guides](how-to-guides.md)**
- Every flag: **[Reference — commands](reference/commands.md)**
- What a code or exit status means: **[Reference — exit codes](reference/exit-codes.md)**
