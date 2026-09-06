import { useEffect, useState } from "react";
import {
  Button, TextArea, TextInput, InlineNotification, Loading, Tag,
} from "@carbon/react";
import { api, type DraftStatus } from "./api";

/* The node-anchored layer editor (decision 3A; register #16). Editing is the write path: the author
 * changes a config layer, the declared-write panel states exactly what a commit would write, and
 * "Propose" opens a *named change* (a draft — the ref is derived, never typed), stages the edit onto
 * it without touching the working tree, and reviews it for semantic conflicts. Apply is the Git host's
 * PR merge, gated by CODEOWNERS — the Studio never merges. */

type Phase =
  | { t: "loading" }
  | { t: "editing" }
  | { t: "naming" }
  | { t: "proposing" }
  | { t: "proposed"; ref: string; status: DraftStatus }
  | { t: "error"; message: string };

export function LayerEditor({
  profile, path, scope, nodes, components, onProposed,
}: { profile: string; path: string; scope: string; nodes: number; components: number; onProposed?: () => void }) {
  const [phase, setPhase] = useState<Phase>({ t: "loading" });
  const [original, setOriginal] = useState("");
  const [text, setText] = useState("");
  const [title, setTitle] = useState("");

  useEffect(() => {
    setPhase({ t: "loading" });
    api.layer(path)
      .then((l) => { setOriginal(l.contents); setText(l.contents); setPhase({ t: "editing" }); })
      .catch((e: Error) => setPhase({ t: "error", message: e.message }));
  }, [path]);

  const dirty = text !== original;
  // A layer is a JSON document; catch a malformed edit here, with a clear message, rather than let
  // the render pipeline reject it downstream as an opaque failure.
  const jsonError = (() => {
    if (!dirty) return null;
    try { JSON.parse(text); return null; } catch (e) { return (e as Error).message; }
  })();

  async function propose() {
    setPhase({ t: "proposing" });
    try {
      const draft = await api.openDraft(title.trim());
      await api.editLayer(draft.ref, path, text);
      const status = await api.draftStatus(draft.ref, profile);
      setPhase({ t: "proposed", ref: draft.ref, status });
      onProposed?.();
    } catch (e) {
      setPhase({ t: "error", message: (e as Error).message });
    }
  }

  if (phase.t === "loading") return <Loading small withOverlay={false} />;
  if (phase.t === "error") {
    return <InlineNotification kind="error" title="Editor unavailable" subtitle={phase.message} hideCloseButton />;
  }

  if (phase.t === "proposed") {
    const s = phase.status;
    return (
      <div className="ec-editor">
        <InlineNotification
          kind="success" lowContrast hideCloseButton
          title="Change proposed"
          subtitle={`Draft ${phase.ref} — the ref is derived; you never type it.`}
        />
        {s.clean ? (
          <InlineNotification kind="info" lowContrast hideCloseButton title="No conflicts"
            subtitle="This change applies onto the current base with no conflict." />
        ) : (
          <>
            {s.textual.length > 0 && (
              <InlineNotification kind="error" lowContrast hideCloseButton title="Textual conflicts"
                subtitle={`Git cannot merge: ${s.textual.join(", ")}. Resolve these first.`} />
            )}
            {s.semantic.length > 0 && (
              <div className="ec-note ec-note--warn">
                <strong>Semantic conflicts</strong> — the merged deployment differs from what you reviewed:
                <ul>
                  {s.semantic.map((c) => (
                    <li key={c.path}><Tag type="red">{c.kind}</Tag> {c.summary}</li>
                  ))}
                </ul>
              </div>
            )}
          </>
        )}
        <p className="ec-sub">
          Submit it for review to open a review request on your Git host, where its code owners gate
          the merge — the Studio never merges.
        </p>
      </div>
    );
  }

  return (
    <div className="ec-editor">
      <TextArea
        labelText={`Editing ${path}`}
        value={text}
        rows={12}
        onChange={(e) => setText(e.target.value)}
        className="ec-editor__area"
      />

      {jsonError && (
        <InlineNotification kind="error" lowContrast hideCloseButton
          title="Not valid JSON" subtitle={jsonError} />
      )}

      <div className="ec-writes">
        <span><strong>Writes</strong> <code>{path}</code></span>
        <span><strong>Scope</strong> {scope}</span>
        <span><strong>Blast radius</strong> {nodes} node(s) × {components} component(s)</span>
      </div>

      {phase.t === "naming" ? (
        <div className="ec-propose">
          <TextInput
            id="draft-title"
            labelText="Name this change"
            placeholder="e.g. Lower the publish interval on the filling line"
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
          <div className="ec-propose__actions">
            <Button kind="secondary" size="sm" onClick={() => setPhase({ t: "editing" })}>Cancel</Button>
            <Button size="sm" disabled={!title.trim()} onClick={propose}>Propose change</Button>
          </div>
        </div>
      ) : (
        <div className="ec-propose__actions">
          <Button
            size="sm"
            disabled={!dirty || jsonError !== null || phase.t === "proposing"}
            onClick={() => setPhase({ t: "naming" })}
          >
            {phase.t === "proposing" ? "Proposing…" : "Propose change…"}
          </Button>
          {!dirty && <span className="ec-sub">Edit the layer above to propose a change.</span>}
        </div>
      )}
    </div>
  );
}
