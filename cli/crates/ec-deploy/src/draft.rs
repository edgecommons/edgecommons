//! Drafts and semantic conflict detection (DESIGN-cli §8.10; Studio register #16, resolving W8).
//!
//! A **draft** is a *named change*, not a branch the user manages. The author names the change; the
//! Studio derives a ref (`draft/<slug>-<id>`) and the vocabulary is **propose → review → apply**. The
//! branch is an implementation detail the author never types.
//!
//! The load-bearing rule is #16 ruling 3: **conflict detection is semantic, at the effective-config
//! level, never textual.** Because [`render`](crate::render::render) is a deterministic pure function
//! of the files at a commit, the Studio renders four points — `base` (where the draft started),
//! `draft` (what the author reviewed), `main` (where the base has moved to), and `merged`
//! (`git merge-tree` of draft and main) — and compares their **outputs**. A merge that changes the
//! effective config is caught even when Git finds no textual conflict: the canonical case is two drafts
//! touching *different* files — one re-parents a node, another edits the layer at its old scope — which
//! merges cleanly yet renders differently for that node.
//!
//! This module is the pure half: the naming rule, and [`detect_conflicts`] over four render outputs. The
//! Git plumbing that produces those outputs (create the draft branch, commit an edit, compute the merged
//! tree) lives behind the [`DraftPort`](crate::ports::DraftPort) and its local adapter.

use std::collections::BTreeMap;

use crate::render::RenderOutput;

/// A draft's stable identity: the author's title, plus the derived ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftName {
    /// What the author typed — the change's human identity, shown everywhere the ref is not.
    pub title: String,
    /// The derived branch ref (`draft/<slug>-<id>`). An implementation detail; never authored.
    pub git_ref: String,
}

impl DraftName {
    /// Derive a draft ref from a title and a short opaque id. The id disambiguates two drafts with the
    /// same title; it is supplied by the caller (from the identity/clock ports) so this stays pure.
    #[must_use]
    pub fn derive(title: &str, id: &str) -> Self {
        let slug = slugify(title);
        let stem = if slug.is_empty() {
            "change".to_string()
        } else {
            slug
        };
        DraftName {
            title: title.trim().to_string(),
            git_ref: format!("draft/{stem}-{id}"),
        }
    }
}

/// Kebab-case a title for a ref: lowercase, runs of non-alphanumerics become single dashes, and the
/// result is trimmed of leading/trailing dashes and capped so the ref stays readable.
#[must_use]
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.chars()
        .take(48)
        .collect::<String>()
        .trim_end_matches('-')
        .to_string()
}

/// Why a merged render diverges from what the author reviewed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictKind {
    /// The draft changed this rendered file, and the merge produced something other than the version
    /// the author reviewed — the base moved under a file the draft also touched.
    Altered,
    /// The draft did *not* change this rendered file, yet merging the draft in changes it relative to
    /// current `main` — the draft's effect reaches a file the author never edited (e.g. a node the
    /// draft re-parents, or whose scope chain another change moved).
    SideEffect,
}

/// One divergence between the reviewed render and the render that would actually deploy.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Conflict {
    /// The rendered-output path (e.g. `gw-fill-01/config-catalog.json`).
    pub path: String,
    pub kind: ConflictKind,
    /// A one-line, human summary — the honest framing from #16 ruling 4.
    pub summary: String,
}

fn index(output: &RenderOutput) -> BTreeMap<&str, &str> {
    output
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.text.as_str()))
        .collect()
}

/// Detect semantic conflicts by comparing render outputs (register #16 ruling 3).
///
/// For every path that appears in any of the four renders, the author reviewed `draft`, so the merge is
/// *expected* to be the draft's version where the draft changed the path, and current `main`'s version
/// everywhere else. Any path where the actual `merged` render differs from that expectation is a
/// conflict — surfaced for a human, never auto-resolved (#16 ruling 5). An empty result means the draft
/// applies onto current main without changing any output the author did not review.
#[must_use]
pub fn detect_conflicts(
    base: &RenderOutput,
    draft: &RenderOutput,
    main: &RenderOutput,
    merged: &RenderOutput,
) -> Vec<Conflict> {
    let (b, d, m, g) = (index(base), index(draft), index(main), index(merged));
    let mut paths: Vec<&str> = b
        .keys()
        .chain(d.keys())
        .chain(m.keys())
        .chain(g.keys())
        .copied()
        .collect();
    paths.sort_unstable();
    paths.dedup();

    let mut conflicts = Vec::new();
    for path in paths {
        let draft_changed = d.get(path) != b.get(path);
        let expected = if draft_changed {
            d.get(path)
        } else {
            m.get(path)
        };
        let actual = g.get(path);
        if actual != expected {
            let kind = if draft_changed {
                ConflictKind::Altered
            } else {
                ConflictKind::SideEffect
            };
            let summary = match kind {
                ConflictKind::Altered => format!(
                    "{path}: your change to this output no longer produces the result you reviewed — the base moved underneath it"
                ),
                ConflictKind::SideEffect => format!(
                    "{path}: your change now alters this output, which you did not edit — another change moved its inputs"
                ),
            };
            conflicts.push(Conflict {
                path: path.to_string(),
                kind,
                summary,
            });
        }
    }
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Plan;
    use crate::render::RenderedFile;

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

    #[test]
    fn ref_is_derived_from_the_title_never_typed() {
        let n = DraftName::derive("Add file-replicator to the filling line", "7f3a");
        assert_eq!(n.title, "Add file-replicator to the filling line");
        assert_eq!(
            n.git_ref,
            "draft/add-file-replicator-to-the-filling-line-7f3a"
        );
    }

    #[test]
    fn slugify_collapses_punctuation_and_trims() {
        assert_eq!(slugify("  Fix: OPC-UA endpoint!! "), "fix-opc-ua-endpoint");
        assert_eq!(slugify("***"), "");
        assert_eq!(DraftName::derive("***", "01").git_ref, "draft/change-01");
    }

    #[test]
    fn a_draft_applying_cleanly_onto_moved_main_has_no_conflict() {
        // The draft edits node A's output; main independently edits node B's. They do not interact.
        let base = out(&[("A/c.json", "a0"), ("B/c.json", "b0")]);
        let draft = out(&[("A/c.json", "a1"), ("B/c.json", "b0")]); // author changed A
        let main = out(&[("A/c.json", "a0"), ("B/c.json", "b1")]); // main changed B
        let merged = out(&[("A/c.json", "a1"), ("B/c.json", "b1")]); // both land
        assert!(detect_conflicts(&base, &draft, &main, &merged).is_empty());
    }

    #[test]
    fn the_reparent_case_is_caught_though_git_sees_no_textual_conflict() {
        // #16's canonical example. The draft edits the layer at A's *old* scope, changing A's render.
        // Meanwhile main re-parents A, so A no longer uses that layer — the merge is textually clean
        // (different files) but A now renders as main produced it, not as the author reviewed.
        let base = out(&[("A/c.json", "old-scope")]);
        let draft = out(&[("A/c.json", "old-scope+edit")]); // author reviewed this
        let main = out(&[("A/c.json", "new-scope")]); // A re-parented
        let merged = out(&[("A/c.json", "new-scope")]); // edit doesn't reach A anymore
        let c = detect_conflicts(&base, &draft, &main, &merged);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "A/c.json");
        assert_eq!(c[0].kind, ConflictKind::Altered);
    }

    #[test]
    fn a_side_effect_on_an_unedited_output_is_a_conflict() {
        // The draft edited nothing that should touch B, yet the merged render changes B relative to
        // main — the draft's effect reached an output the author never reviewed.
        let base = out(&[("A/c.json", "a0"), ("B/c.json", "b0")]);
        let draft = out(&[("A/c.json", "a1"), ("B/c.json", "b0")]); // author only changed A
        let main = out(&[("A/c.json", "a0"), ("B/c.json", "b0")]); // main unchanged
        let merged = out(&[("A/c.json", "a1"), ("B/c.json", "b-surprise")]); // B moved anyway
        let c = detect_conflicts(&base, &draft, &main, &merged);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "B/c.json");
        assert_eq!(c[0].kind, ConflictKind::SideEffect);
    }

    #[test]
    fn both_drafts_editing_the_same_output_conflicts_when_the_merge_matches_neither_review() {
        let base = out(&[("A/c.json", "0")]);
        let draft = out(&[("A/c.json", "draft-version")]);
        let main = out(&[("A/c.json", "main-version")]);
        let merged = out(&[("A/c.json", "textually-merged")]); // neither reviewed this
        let c = detect_conflicts(&base, &draft, &main, &merged);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].kind, ConflictKind::Altered);
    }

    #[test]
    fn an_added_file_that_survives_the_merge_intact_is_clean() {
        let base = out(&[("A/c.json", "a0")]);
        let draft = out(&[("A/c.json", "a0"), ("A/new.json", "n")]); // author added a file
        let main = out(&[("A/c.json", "a0")]);
        let merged = out(&[("A/c.json", "a0"), ("A/new.json", "n")]);
        assert!(detect_conflicts(&base, &draft, &main, &merged).is_empty());
    }

    #[test]
    fn a_draft_addition_clobbered_by_main_is_a_conflict() {
        let base = out(&[("A/c.json", "a0")]);
        let draft = out(&[("A/c.json", "a0"), ("A/new.json", "draft-add")]);
        let main = out(&[("A/c.json", "a0"), ("A/new.json", "main-add")]); // main added it too
        let merged = out(&[("A/c.json", "a0"), ("A/new.json", "main-add")]);
        let c = detect_conflicts(&base, &draft, &main, &merged);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].path, "A/new.json");
        assert_eq!(c[0].kind, ConflictKind::Altered);
    }
}
