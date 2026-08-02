//! `deployment diff` — the delta between two renders, **grouped by consequence** (DESIGN-cli §8.1).
//!
//! An operator does not want a file-level delta; they want to know *what will happen to the plant*:
//! what restarts, what changes on the wire, what identity or permission moves, what storage is
//! touched. So a diff classifies every changed rendered artifact into exactly one
//! [`Consequence`] bucket, and reports the buckets.
//!
//! This is also the first stage of the drift taxonomy (Studio decision 6A): **definition → release**.
//! Later stages (delivery, runtime) need evidence from a control plane and are a separate cut; this
//! one is a pure function of two renders, so it runs offline with no network and no credentials.
//!
//! # Classification, and its honest limits
//!
//! The renderers emit different shapes per platform, so classification uses the most specific signal
//! available for each:
//!
//! | Rendered artifact | Bucket | Why |
//! |---|---|---|
//! | `<node>/supervisord.conf` | `Restart` (`ApplyOrder` if only ordering moved) | the supervisor bundle is the process contract |
//! | `<node>/*-messaging.json` | `Network` | broker/client-id/timeouts are wire-visible |
//! | `<node>/config-catalog.json`, `config-component-config.json` | `Config`, or `Restart` when a component on that node does not hot-reload | restart impact follows the **config source**, never the platform (D-CLI-14) |
//! | `<node>/<component>.yaml` (Kubernetes) | per embedded `kind:` — `ServiceAccount`→`Identity`, `Role`/`RoleBinding`→`Permission`, `Service`→`Network`, `PersistentVolumeClaim`→`Storage`, `ConfigMap`→`Config`, `Deployment`→`Restart` | one file carries several objects; the object kind is the real signal |
//! | `<node>/deployment.json` (Greengrass) | `Artifact` when a `componentVersion` moved, else `Config` | the deployment document fuses both streams on the wire, but the model does not (D-CLI-13) |
//! | `requirements.json`, `plan.json` | not reported | derived summaries of the above; reporting them would double-count |
//!
//! Where a node-level artifact (the catalog) serves several components with different config
//! sources, the classification is **conservative**: if any component on that node would restart, the
//! change is reported as `Restart`. Telling an operator "no restart" when one component does restart
//! is the failure that matters; the reverse is merely noisy.

use std::collections::{BTreeMap, BTreeSet};

use crate::render::RenderOutput;
use crate::{Consequence, Plan};

/// Whether a rendered artifact appeared, disappeared, or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
}

/// One classified change between two renders.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// The node the artifact belongs to, when the path carries one (`<node>/…`).
    pub node: Option<String>,
    /// The rendered artifact's path.
    pub path: String,
    pub kind: ChangeKind,
    pub consequence: Consequence,
    /// A one-line, operator-facing statement of what this change does.
    pub summary: String,
}

/// The delta between two renders, grouped by consequence.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffReport {
    pub changes: Vec<Change>,
}

impl DiffReport {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The changes bucketed by consequence, in the order an operator reads them: what breaks running
    /// processes first, bookkeeping last.
    #[must_use]
    pub fn by_consequence(&self) -> BTreeMap<Consequence, Vec<&Change>> {
        let mut out: BTreeMap<Consequence, Vec<&Change>> = BTreeMap::new();
        for c in &self.changes {
            out.entry(c.consequence).or_default().push(c);
        }
        out
    }

    /// Every node touched by this delta.
    #[must_use]
    pub fn nodes(&self) -> BTreeSet<&str> {
        self.changes
            .iter()
            .filter_map(|c| c.node.as_deref())
            .collect()
    }
}

/// Artifacts that are derived summaries of other artifacts. Reporting them would double-count every
/// change that already appears through the artifact it summarises.
const DERIVED: &[&str] = &["requirements.json", "plan.json"];

fn index(o: &RenderOutput) -> BTreeMap<&str, &str> {
    o.files
        .iter()
        .map(|f| (f.path.as_str(), f.text.as_str()))
        .collect()
}

/// The node a rendered path belongs to (`<node>/<file>`), or `None` for a workspace-level artifact.
fn node_of(path: &str) -> Option<&str> {
    path.split_once('/').map(|(node, _)| node)
}

/// Whether any component on this node would restart when its config changes. Derived from the plan's
/// per-component restart impact, which follows the config source (D-CLI-14) — never assumed.
fn node_restarts_on_config(plan: &Plan, node: &str) -> bool {
    plan.entries
        .iter()
        .any(|e| e.node == node && e.consequence == Consequence::Config && e.restarts_component)
}

/// The Kubernetes object kinds present in a rendered manifest, in file order.
fn k8s_kinds(text: &str) -> Vec<&str> {
    text.lines()
        .filter_map(|l| l.strip_prefix("kind:"))
        .map(str::trim)
        .collect()
}

/// The consequence of a change to a Kubernetes manifest: the most disruptive kind whose section
/// actually differs. Comparing whole files would report `Restart` for a pure `Service` edit.
fn k8s_consequence(before: Option<&str>, after: &str) -> (Consequence, &'static str) {
    let changed: Vec<String> = match before {
        // A new manifest brings every object with it; rank by its most disruptive kind.
        None => k8s_kinds(after).into_iter().map(str::to_string).collect(),
        Some(b) => {
            let (bs, as_) = (k8s_sections(b), k8s_sections(after));
            let mut kinds: Vec<String> = Vec::new();
            for (kind, body) in &as_ {
                if bs.get(kind) != Some(body) {
                    kinds.push(kind.clone());
                }
            }
            for kind in bs.keys() {
                if !as_.contains_key(kind) {
                    kinds.push(kind.clone());
                }
            }
            kinds
        }
    };
    // Most disruptive wins: a manifest whose Deployment and Service both moved is a restart.
    let rank = |k: &str| match k {
        "Deployment" | "StatefulSet" | "DaemonSet" => 6,
        "PersistentVolumeClaim" => 5,
        "Role" | "RoleBinding" | "ClusterRole" | "ClusterRoleBinding" => 4,
        "ServiceAccount" => 3,
        "Service" => 2,
        "ConfigMap" | "Secret" => 1,
        _ => 0,
    };
    let top = changed
        .iter()
        .max_by_key(|k| rank(k))
        .map_or("", String::as_str);
    match top {
        "Deployment" | "StatefulSet" | "DaemonSet" => (Consequence::Restart, "workload spec"),
        "PersistentVolumeClaim" => (Consequence::Storage, "volume claim"),
        "Role" | "RoleBinding" | "ClusterRole" | "ClusterRoleBinding" => {
            (Consequence::Permission, "RBAC rules")
        }
        "ServiceAccount" => (Consequence::Identity, "service account"),
        "Service" => (Consequence::Network, "service"),
        "ConfigMap" | "Secret" => (Consequence::Config, "config data"),
        _ => (Consequence::Config, "manifest"),
    }
}

/// Split a multi-document Kubernetes manifest into `kind → body`. Objects are keyed by kind because
/// the renderer emits at most one object of each kind per component manifest.
fn k8s_sections(text: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for doc in text.split("\n---") {
        let kind = doc
            .lines()
            .find_map(|l| l.strip_prefix("kind:"))
            .map(str::trim)
            .unwrap_or_default();
        if !kind.is_empty() {
            out.insert(kind.to_string(), doc.to_string());
        }
    }
    out
}

/// Every `componentVersion` a Greengrass deployment document pins, collected **structurally** — the
/// artifact stream — so it can be told apart from a `configurationUpdate`-only change, which is the
/// config stream (D-CLI-13). Parsed rather than line-matched: the renderer may emit the whole
/// document on one line, where a textual compare cannot separate the two streams at all.
fn gg_versions(text: &str) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if k == "componentVersion" {
                        out.push(val.as_str().unwrap_or_default().to_string());
                    }
                    walk(val, out);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(|i| walk(i, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        walk(&v, &mut out);
    }
    out.sort();
    out
}

/// A supervisord bundle parsed into `[section] → body`, with `priority=` dropped. Keying by section
/// and sorting (a `BTreeMap`) makes the comparison order-insensitive, which is what lets a pure
/// re-ordering be told apart from a change to the commands or supervision settings that actually
/// restart a process — the renderer emits program blocks *in* priority order, so a reorder moves the
/// blocks as well as the `priority=` values.
fn supervisord_sections(text: &str) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            current = t.to_string();
            out.entry(current.clone()).or_default();
        } else if !current.is_empty() && !t.starts_with("priority=") && !t.is_empty() {
            out.entry(current.clone()).or_default().push(t.to_string());
        }
    }
    out
}

/// Whether a supervisord bundle differs only in program ordering, rather than in the commands or
/// supervision settings that actually restart a process.
///
/// If either side parses to no sections at all — a shape this parser does not understand — the
/// answer is `false`, so the change is reported as a restart. Over-reporting a restart is noise;
/// under-reporting one lets a process bounce that the operator was told would not.
fn only_ordering_moved(before: &str, after: &str) -> bool {
    let (b, a) = (supervisord_sections(before), supervisord_sections(after));
    !b.is_empty() && !a.is_empty() && before != after && b == a
}

/// Classify one changed artifact.
fn classify(
    path: &str,
    before: Option<&str>,
    after: Option<&str>,
    plan_after: &Plan,
) -> (Consequence, String) {
    let node = node_of(path).unwrap_or_default();
    let file = path.rsplit_once('/').map_or(path, |(_, f)| f);

    // Kubernetes: one manifest carries several objects; the changed object's kind is the signal.
    if file.ends_with(".yaml") {
        if let Some(a) = after {
            let (c, what) = k8s_consequence(before, a);
            return (c, format!("{node}: {what} changed"));
        }
        return (Consequence::Restart, format!("{node}: manifest removed"));
    }

    // Greengrass: the deployment document fuses both streams on the wire; separate them here.
    if file == "deployment.json" {
        if let (Some(b), Some(a)) = (before, after) {
            if gg_versions(b) != gg_versions(a) {
                return (
                    Consequence::Artifact,
                    format!("{node}: component version(s) moved"),
                );
            }
            return (
                Consequence::Config,
                format!("{node}: deployment configuration changed"),
            );
        }
        return (
            Consequence::Artifact,
            format!("{node}: deployment document {}", added_or_removed(before)),
        );
    }

    // HOST: the supervisor bundle is the process contract.
    if file == "supervisord.conf" {
        if let (Some(b), Some(a)) = (before, after) {
            if only_ordering_moved(b, a) {
                return (
                    Consequence::ApplyOrder,
                    format!("{node}: launch order moved"),
                );
            }
        }
        return (
            Consequence::Restart,
            format!("{node}: supervisor bundle changed"),
        );
    }

    // Messaging files are wire-visible: broker, client id, timeouts.
    if file.ends_with("-messaging.json") {
        return (Consequence::Network, format!("{node}: messaging changed"));
    }

    // Effective config. Restart impact follows the config source, via the plan — never assumed.
    if file == "config-catalog.json" || file == "config-component-config.json" {
        if node_restarts_on_config(plan_after, node) {
            return (
                Consequence::Restart,
                format!("{node}: config changed and a component there does not hot-reload"),
            );
        }
        return (
            Consequence::Config,
            format!("{node}: config changed (hot-reload)"),
        );
    }

    (Consequence::Config, format!("{node}: {file} changed"))
}

fn added_or_removed(before: Option<&str>) -> &'static str {
    if before.is_some() { "removed" } else { "added" }
}

/// Diff two renders and group the delta by consequence (DESIGN-cli §8.1).
///
/// `plan_after` supplies restart impact per component, so a config change is classified by the
/// component's **config source** rather than by the platform. Derived summaries (`plan.json`,
/// `requirements.json`) are skipped: every change they summarise is already reported through the
/// artifact that produced it.
#[must_use]
pub fn diff_renders(before: &RenderOutput, after: &RenderOutput, plan_after: &Plan) -> DiffReport {
    let (b, a) = (index(before), index(after));
    let mut paths: Vec<&str> = b.keys().chain(a.keys()).copied().collect();
    paths.sort_unstable();
    paths.dedup();

    let mut changes = Vec::new();
    for path in paths {
        if DERIVED.contains(&path) {
            continue;
        }
        let (bt, at) = (b.get(path).copied(), a.get(path).copied());
        let kind = match (bt, at) {
            (Some(x), Some(y)) if x == y => continue,
            (Some(_), Some(_)) => ChangeKind::Modified,
            (None, Some(_)) => ChangeKind::Added,
            (Some(_), None) => ChangeKind::Removed,
            (None, None) => continue,
        };
        let (consequence, summary) = classify(path, bt, at, plan_after);
        changes.push(Change {
            node: node_of(path).map(str::to_string),
            path: path.to_string(),
            kind,
            consequence,
            summary,
        });
    }
    DiffReport { changes }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::RenderedFile;
    use crate::{Consequence, PlanEntry};

    fn out(files: &[(&str, &str)]) -> RenderOutput {
        RenderOutput {
            files: files
                .iter()
                .map(|(p, t)| RenderedFile {
                    path: (*p).to_string(),
                    text: (*t).to_string(),
                })
                .collect(),
            plan: Plan::default(),
        }
    }

    /// A plan whose single config entry declares whether that node's component restarts.
    fn plan(node: &str, restarts: bool) -> Plan {
        Plan {
            entries: vec![PlanEntry {
                node: node.into(),
                component: "telemetry-processor".into(),
                consequence: Consequence::Config,
                summary: "deliver effective config".into(),
                restarts_component: restarts,
            }],
        }
    }

    #[test]
    fn an_identical_render_has_no_delta() {
        let r = out(&[("gw/config-catalog.json", "a")]);
        assert!(diff_renders(&r, &r, &Plan::default()).is_empty());
    }

    #[test]
    fn derived_summaries_are_not_double_counted() {
        // plan.json and requirements.json summarise changes reported through other artifacts.
        let before = out(&[("plan.json", "1"), ("requirements.json", "1")]);
        let after = out(&[("plan.json", "2"), ("requirements.json", "2")]);
        assert!(diff_renders(&before, &after, &Plan::default()).is_empty());
    }

    #[test]
    fn a_config_change_is_config_when_the_component_hot_reloads() {
        let before = out(&[("gw/config-catalog.json", "old")]);
        let after = out(&[("gw/config-catalog.json", "new")]);
        let d = diff_renders(&before, &after, &plan("gw", false));
        assert_eq!(d.changes.len(), 1);
        assert_eq!(d.changes[0].consequence, Consequence::Config);
        assert_eq!(d.changes[0].kind, ChangeKind::Modified);
        assert_eq!(d.changes[0].node.as_deref(), Some("gw"));
    }

    #[test]
    fn the_same_config_change_is_a_restart_when_a_component_there_does_not_hot_reload() {
        // Restart impact follows the config source (D-CLI-14), read from the plan — not the platform.
        let before = out(&[("gw/config-catalog.json", "old")]);
        let after = out(&[("gw/config-catalog.json", "new")]);
        let d = diff_renders(&before, &after, &plan("gw", true));
        assert_eq!(d.changes[0].consequence, Consequence::Restart);
        assert!(d.changes[0].summary.contains("does not hot-reload"));
    }

    #[test]
    fn messaging_is_network_and_supervisor_is_restart() {
        let before = out(&[
            ("gw/telemetry-messaging.json", "old"),
            ("gw/supervisord.conf", "[program:t]\ncommand=a\npriority=10"),
        ]);
        let after = out(&[
            ("gw/telemetry-messaging.json", "new"),
            ("gw/supervisord.conf", "[program:t]\ncommand=b\npriority=10"),
        ]);
        let d = diff_renders(&before, &after, &Plan::default());
        let by = d.by_consequence();
        assert_eq!(by[&Consequence::Network].len(), 1);
        assert_eq!(by[&Consequence::Restart].len(), 1);
    }

    #[test]
    fn a_pure_reordering_of_the_supervisor_bundle_is_apply_order_not_restart() {
        // The real renderer emits program blocks *in* priority order, so a reorder moves whole
        // blocks as well as the `priority=` values. Use that shape, not a one-line stand-in.
        let a = "[program:emqx]\ncommand=/usr/bin/emqx foreground\npriority=10\n\n\
                 [program:telemetry]\ncommand=/opt/telemetry\npriority=30\n";
        let b = "[program:telemetry]\ncommand=/opt/telemetry\npriority=10\n\n\
                 [program:emqx]\ncommand=/usr/bin/emqx foreground\npriority=30\n";
        let d = diff_renders(
            &out(&[("gw/supervisord.conf", a)]),
            &out(&[("gw/supervisord.conf", b)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::ApplyOrder);
    }

    #[test]
    fn a_changed_command_in_the_supervisor_bundle_is_a_restart_not_apply_order() {
        let a = "[program:telemetry]\ncommand=/opt/telemetry --old\npriority=30\n";
        let b = "[program:telemetry]\ncommand=/opt/telemetry --new\npriority=30\n";
        let d = diff_renders(
            &out(&[("gw/supervisord.conf", a)]),
            &out(&[("gw/supervisord.conf", b)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Restart);
    }

    #[test]
    fn kubernetes_classifies_by_the_object_kind_that_actually_moved() {
        // Only the Service section differs: a whole-file compare would wrongly say Restart.
        let dep = "kind: Deployment\nspec: same\n---\nkind: Service\nport: 80";
        let dep2 = "kind: Deployment\nspec: same\n---\nkind: Service\nport: 443";
        let d = diff_renders(
            &out(&[("gw/telemetry.yaml", dep)]),
            &out(&[("gw/telemetry.yaml", dep2)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Network);

        // The workload spec moving is a restart, and outranks a simultaneous ConfigMap edit.
        let a = "kind: ConfigMap\ndata: x\n---\nkind: Deployment\nimage: v1";
        let b = "kind: ConfigMap\ndata: y\n---\nkind: Deployment\nimage: v2";
        let d = diff_renders(
            &out(&[("gw/telemetry.yaml", a)]),
            &out(&[("gw/telemetry.yaml", b)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Restart);

        // Identity and permission are distinguishable from config.
        let sa1 = "kind: ServiceAccount\nname: a";
        let sa2 = "kind: ServiceAccount\nname: b";
        let d = diff_renders(
            &out(&[("gw/x.yaml", sa1)]),
            &out(&[("gw/x.yaml", sa2)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Identity);
    }

    #[test]
    fn greengrass_separates_the_two_streams_even_though_the_wire_fuses_them() {
        let cfg_only = (
            "{\"componentVersion\": \"1.0.0\", \"merge\": \"a\"}",
            "{\"componentVersion\": \"1.0.0\", \"merge\": \"b\"}",
        );
        let d = diff_renders(
            &out(&[("thing/deployment.json", cfg_only.0)]),
            &out(&[("thing/deployment.json", cfg_only.1)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Config);

        let version_moved = (
            "{\"componentVersion\": \"1.0.0\"}",
            "{\"componentVersion\": \"1.1.0\"}",
        );
        let d = diff_renders(
            &out(&[("thing/deployment.json", version_moved.0)]),
            &out(&[("thing/deployment.json", version_moved.1)]),
            &Plan::default(),
        );
        assert_eq!(d.changes[0].consequence, Consequence::Artifact);
    }

    #[test]
    fn added_and_removed_artifacts_are_reported_with_their_node() {
        let d = diff_renders(
            &out(&[("gw-a/supervisord.conf", "x")]),
            &out(&[("gw-b/supervisord.conf", "y")]),
            &Plan::default(),
        );
        assert_eq!(d.changes.len(), 2);
        let kinds: BTreeSet<ChangeKind> = d.changes.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&ChangeKind::Added));
        assert!(kinds.contains(&ChangeKind::Removed));
        assert_eq!(d.nodes().len(), 2);
    }
}
