# Explanation — How this scaffold is shaped, and why

*This documents the generated scaffold; rewrite it as you build the component out.*

This page is the mental model behind the generated code. For exact options see
[reference/](reference/); for tasks, the [how-to guides](how-to-guides.md).

## What is automatic, and what this scaffold adds

The library gives every component two things with **zero code**: the `state` keepalive
(`ecv1/{device}/{component}/main/state`, on/5s/local by default) and the command inbox
(`ping`/`reload-config`/`get-configuration`, live before `App::new` even runs). What `src/app.rs`
adds on top is the rest of the monitoring/command surface an edge-console reads — a metric, a data
signal, an event, an instance-connectivity provider, and a custom command verb — so a freshly
generated component has something live to observe and command, instead of an empty dashboard. None
of it is required by the library; a bare scaffold with no `App::new` additions works fine. It exists
so the demonstrated surface is live end-to-end out of the box, and so you have a concrete pattern to
copy when you add your own.

## Facades, not raw publish

Every demo publish goes through a facade — `gg.data()`, `gg.events()`, `gg.metrics()` — never a
hand-built topic string or envelope. `DataFacade` constructs the `SouthboundSignalUpdate` body and
mints its topic from the signal id; `EventsFacade` derives the `evt/{severity}/{type}` channel from
the arguments you pass, so the topic and the body can never disagree. This is the same discipline
the southbound-adapter archetype (`protocol-adapter` kind) enforces more strictly (an allow-listed
write surface, a device seam) — this general-purpose scaffold demonstrates the same facades without
imposing a device model on you.

## Quality is honest by default, not silently assumed

`self.data.publish_value(DATA_SIGNAL_ID, demo_value)` — the one-line path — omits an explicit
quality, and the facade defaults it to `Quality::Good` with `qualityRaw: "unspecified"` (a
synthesized-vs-device-reported marker on the wire). This is correct for a demo sine wave that never
fails to compute; it would be **wrong** for anything that can actually fail to read. When you replace
the demo signal with a real one, decide whether an omitted quality is honest for your source, and use
`publish_value_with_quality` (or the fuller `signal(...)` builder) the moment it is not.

## Instance connectivity: registered, even with nothing to report

`App::new` registers an instance-connectivity provider that returns an empty `Vec` — this scaffold
owns no southbound connections, so it reports none. That is a real answer, not a missing one: the
`state` keepalive's `instances[]` section is omitted, and the built-in `status` verb says exactly
what `ping` says. The provider is registered anyway, precisely so the seam is visible and easy to
extend the day this component grows a connection of its own — see the comment in `App::new` for the
one-provider/two-surface shape (push into the keepalive, pull via `status`) every archetype in this
ecosystem shares.

## Config hot-reload: a listener, not a poll

`ConfigListener` implements `ConfigurationChangeListener` and is registered before anything else in
`App::new`. The library calls it whenever the deployment config changes at runtime (a Greengrass
redeployment, a Kubernetes ConfigMap update) — this scaffold just logs the new identity; put your own
reaction (re-reading a tuning parameter, rebuilding a derived cache) in `on_configuration_change`.

## Commands are installed before the inbox goes active

`configure_commands` runs through `EdgeCommonsBuilder::configure_commands(...)` in `src/main.rs`,
**before** `EdgeCommonsBuilder::build()` returns — so `set-greeting` is registered before the inbox
starts accepting requests, and a command arriving the instant the component comes up cannot race an
unregistered verb. Follow the same pattern for your own verbs: install them in the builder chain, not
after `App::new` returns.
