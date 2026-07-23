# Dallas fixture — a frozen snapshot, not the canonical definition

`definition.yaml`, `bindings/`, and `layers/` here are a **frozen copy** of the Dallas bottling
site's deployment definition. `golden/` is its committed rendered output. Together they are the
kernel's byte-for-byte regression oracle (`../../dallas_golden.rs`, `../../dallas_gg_golden.rs`).

**The canonical definition lives with the site it deploys:**
[`bottling-company-test/sites/dallas-site`](https://github.com/edgecommons/bottling-company-test/tree/main/sites/dallas-site).
That repo's `config-drift-gate` renders its definition and diffs the result against its checked-in
config sources — the definition is the source of truth, the configs are its output.

This copy exists because the kernel needs an in-tree oracle it can test against with no network and no
sibling repo. It must render **identically** to the canonical one. If the two ever diverge, one is
wrong: when the site definition changes intentionally, refresh this snapshot (and its `golden/`) in
the same change, and the golden test will confirm the bytes still match.
