# Split-config cross-language conformance vectors

These files pin the shared-device-layer configuration contract in
`docs/SPLIT_CONFIG_IMPLEMENTATION_SPEC.md`.

All four core libraries must consume `merge.json`, `resolution.json`, and
`config-component-bundles.json`. The dedicated
`com.mbreissi.edgecommons.ConfigComponent` implementation must consume
`config-component-catalogs.json`.

The vectors describe behavior, not a required test harness shape. Languages may map
the cases into their local test idioms as long as the expected results and error
codes are preserved.

