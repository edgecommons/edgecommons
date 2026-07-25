import { useEffect, useState } from "react";
import { Button, Tag, InlineNotification, Loading } from "@carbon/react";
import { api, type DefinitionView, type DraftStatus, type DraftSummary } from "./api";
import { layersForSelection, type Selection } from "./Shell";

/* Draft orchestration (deck ch. 13 slice 3; register #16). Drafts are shown on Overview, keyed on the
 * change's *name*; presence is advisory — "N open drafts touch this scope" — never a lock (ruling 6).
 * Each draft can be reviewed for semantic conflicts (ruling 3) and taken to apply, which is a pull
 * request on the Git host gated by CODEOWNERS — the Studio never merges (ruling 7). */

export function DraftsPanel({
  def, sel, profile, version,
}: { def: DefinitionView; sel: Selection; profile: string; version: number }) {
  const [drafts, setDrafts] = useState<DraftSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDrafts(null); setError(null);
    api.listDrafts().then((r) => setDrafts(r.drafts)).catch((e: Error) => setError(e.message));
  }, [version]);

  if (error) return <InlineNotification kind="error" title="Drafts unavailable" subtitle={error} hideCloseButton />;
  if (!drafts) return <Loading small withOverlay={false} />;
  if (!drafts.length) {
    return <p className="ec-sub">No open drafts. Edit a layer under <strong>Config</strong> to propose a change.</p>;
  }

  const affecting = layersForSelection(def, sel);
  const touches = (d: DraftSummary) => d.changedFiles.some((f) => affecting.has(f));
  const here = drafts.filter(touches);

  return (
    <>
      {here.length > 0 && (
        <div className="ec-note">
          <strong>{here.length} open draft(s) touch this scope.</strong> Advisory, not a lock — you can
          edit here too; a real overlap is reported as a conflict when you review.
        </div>
      )}
      {drafts.map((d) => (
        <DraftRow key={d.ref} draft={d} profile={profile} touches={touches(d)} />
      ))}
    </>
  );
}

/** The advisory presence line for the editing surface: which open drafts already touch this scope,
 *  so an author sees the overlap before editing (register #16 ruling 6). Not a lock. */
export function ScopePresence({
  def, sel, version,
}: { def: DefinitionView; sel: Selection; version: number }) {
  const [drafts, setDrafts] = useState<DraftSummary[] | null>(null);
  useEffect(() => {
    api.listDrafts().then((r) => setDrafts(r.drafts)).catch(() => setDrafts([]));
  }, [version]);
  if (!drafts) return null;
  const affecting = layersForSelection(def, sel);
  const here = drafts.filter((d) => d.changedFiles.some((f) => affecting.has(f)));
  if (!here.length) return null;
  return (
    <div className="ec-note">
      <strong>{here.length} open draft(s) touch this scope:</strong> {here.map((d) => d.title).join("; ")}.
      Advisory, not a lock — editing here is allowed; an overlap surfaces as a conflict on review.
    </div>
  );
}

function DraftRow({
  draft, profile, touches,
}: { draft: DraftSummary; profile: string; touches: boolean }) {
  const [status, setStatus] = useState<DraftStatus | null>(null);
  const [apply, setApply] = useState<{ applied: boolean; url: string | null; reason?: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [applying, setApplying] = useState(false);

  async function review() {
    setBusy(true);
    try { setStatus(await api.draftStatus(draft.ref, profile)); }
    catch (e) { setStatus({ clean: false, textual: [(e as Error).message], semantic: [] }); }
    finally { setBusy(false); }
  }

  // Apply pushes the draft branch and opens its pull request, gated by CODEOWNERS on the host —
  // the Studio never merges. It degrades to a stated instruction when no host is configured.
  async function doApply() {
    setApplying(true);
    try {
      const r = await api.applyDraft(draft.ref, draft.title);
      setApply(r);
      if (r.applied && r.url) window.open(r.url, "_blank", "noopener");
    } catch (e) {
      setApply({ applied: false, url: null, reason: (e as Error).message });
    } finally {
      setApplying(false);
    }
  }

  return (
    <div className="ec-draft">
      <div className="ec-draft__head">
        <strong>{draft.title}</strong>
        {touches && <Tag type="teal">touches this scope</Tag>}
        <code>{draft.ref}</code>
        <span className="ec-draft__meta">{draft.changedFiles.length} file(s) changed</span>
      </div>
      <div className="ec-draft__actions">
        <Button size="sm" kind="tertiary" disabled={busy} onClick={review}>
          {busy ? "Reviewing…" : "Review conflicts"}
        </Button>
        <Button size="sm" kind="ghost" disabled={applying} onClick={doApply}>
          {applying ? "Applying…" : "Apply — open pull request"}
        </Button>
      </div>

      {status && (status.clean ? (
        <InlineNotification kind="info" lowContrast hideCloseButton title="No conflicts"
          subtitle="Applies onto the current base cleanly." />
      ) : (
        <>
          {status.textual.length > 0 && (
            <InlineNotification kind="error" lowContrast hideCloseButton title="Textual conflicts"
              subtitle={`Git cannot merge: ${status.textual.join(", ")}.`} />
          )}
          {status.semantic.length > 0 && (
            <div className="ec-note ec-note--warn">
              <strong>Semantic conflicts</strong> — the merged deployment differs from what was reviewed:
              <ul>{status.semantic.map((c) => (
                <li key={c.path}><Tag type="red">{c.kind}</Tag> {c.summary}</li>
              ))}</ul>
            </div>
          )}
        </>
      ))}

      {apply && (apply.applied ? (
        <InlineNotification kind="success" lowContrast hideCloseButton title="Pull request opened"
          subtitle={apply.url ? `Review and merge on your Git host: ${apply.url}` : "Opened on your Git host."} />
      ) : (
        <p className="ec-sub">
          {apply.reason ?? "Apply unavailable."} The Studio never merges — CODEOWNERS gates the merge.
        </p>
      ))}
    </div>
  );
}
