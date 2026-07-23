# edgecommons

`edgecommons` is a single static binary that builds and ships EdgeCommons components. It scaffolds a
component in Java, Python, Rust, or TypeScript; validates its configuration and packaging against the
canonical schema; moves it between library versions; packages and releases it; and compiles a
deployment definition into the artifacts a target platform actually consumes.

It carries the component templates and the config schema **inside the binary**, so scaffolding and
validation work with no network and no registry. Network access exists in exactly two places, both
named and both opt-in: `deployment lock`, and `component new --template-git`.

```bash
edgecommons component new --name com.example.MyAdapter --language RUST --kind protocol-adapter
edgecommons component validate --platform GREENGRASS
edgecommons deployment render site.yaml --env prod --target HOST
```

## Where to start

- **[Tutorial](tutorial.md)** — scaffold, configure, validate, and package your first component end to
  end. Start here if you have never used the tool.
- **[How-to guides](how-to-guides.md)** — task recipes: upgrade a library version, package for
  Greengrass, render a deployment, wire it into CI, fix the errors you will actually hit.
- **[Reference — commands](reference/commands.md)** — every verb, argument, and flag.
- **[Reference — exit codes and diagnostics](reference/exit-codes.md)** — what each exit code means and
  what every `EC****` diagnostic is telling you.
- **[Explanation](explanation.md)** — why one static binary, why it refuses to touch the network, and
  why it produces artifacts but never publishes them.

## The command surface

Seven verb families, noun first:

| Family | What it does |
|---|---|
| `component` | `new`, `validate`, `upgrade`, `version`, `package`, `release` — the component lifecycle |
| `template` | `list`, `show` — inspect the templates the binary carries |
| `registry` | `list`, `show`, `versions` — query the ecosystem catalog |
| `deployment` | `validate`, `lock`, `render`, `plan`, `diff`, `release` — model to platform artifacts |
| `studio` | `serve` — the Deployment Studio server over the same kernel |
| `doctor` | check the external tools your targets need |
| `completions` | generate a shell completion script |

Global flags work everywhere: `--json` for machine-readable output, `-q/--quiet`, `-v/--verbose`
(repeatable), `--no-color`, and `--yes` to turn a missing prompt into a usage error instead of a
question — which is what you want in CI.

## Requirements

The binary itself has no runtime dependencies. Generating a Java or TypeScript component needs no JVM
and no Node — you need a language toolchain to *build* what it generates, not to generate it. Run
`edgecommons doctor` to see what your chosen targets require.
