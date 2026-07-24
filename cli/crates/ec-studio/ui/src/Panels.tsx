import { useEffect, useState } from "react";
import { Loading, InlineNotification, Tag } from "@carbon/react";
import {
  api, levelOf, valueOf,
  type AccessView, type DefinitionView, type EvidenceView, type RenderView,
} from "./api";
import { selectionScopeId, useTopology, type Selection } from "./Shell";

/* Panels render product state. Design rationale lives in the design repo's mock and REVIEW-UI,
 * never in this surface. */

/** An area that exists in the agreed IA but is not built yet — said plainly, never faked. */
export function NotBuilt({ what, detail }: { what: string; detail: string }) {
  return (
    <div className="ec-empty">
      <strong>{what}</strong>
      {detail}
    </div>
  );
}

function useAsync<T>(fn: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    setData(null); setError(null);
    fn().then((d) => { if (live) setData(d); }).catch((e: Error) => { if (live) setError(e.message); });
    return () => { live = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return { data, error };
}

/* ── Overview ─────────────────────────────────────────────────────────────────────── */

export function Overview({ def, sel, profile }: { def: DefinitionView; sel: Selection; profile: string }) {
  const t = useTopology(def);
  const { data: ev } = useAsync(() => api.evidence(profile), [profile]);

  const scopeId = selectionScopeId(sel, def);
  const nodes = sel.kind === "node"
    ? def.nodes.filter((n) => n.key === sel.id)
    : scopeId ? t.nodesUnder(scopeId) : [];
  const comps = nodes.reduce((a, n) => a + n.components.length, 0);
  const chain = scopeId ? t.chainOf(scopeId) : [];

  const artifacts = ev?.manifest.streams.artifact ?? [];
  const mine = artifacts.filter((a) => nodes.some((n) => n.key === a.node));
  const unpinned = mine.filter((a) => !a.version).length;

  return (
    <>
      <h2>Aggregates</h2>
      <div className="ec-cards">
        <Card label="Nodes" value={nodes.length} note="at or beneath this selection" />
        <Card label="Component assignments" value={comps} note="config leaves merged here" />
        <Card label="Layer chain depth" value={chain.length}
              note={`${chain.map((c) => levelOf(c.id)).join(" → ")} → leaf`} />
        <Card label="Unpinned artifacts" value={ev ? unpinned : "—"}
              note={ev ? (unpinned ? "source-form; blocks protected promotion" : "all pinned to version + digest") : "loading"} />
      </div>

      <h2>Streams</h2>
      {!ev ? <Loading small withOverlay={false} /> : (
        <div className="ec-streams">
          <div className="ec-stream ec-stream--config">
            <div className="ec-stream__head">
              <Tag type="teal">config</Tag><strong>{ev.streamTags.config}</strong>
            </div>
            <dl className="ec-kv">
              <dt>Delivery</dt>
              <dd>{[...new Set(mine.map((a) => a.configSource))].join(", ") || "—"}</dd>
              <dt>Restart impact</dt>
              <dd>{mine.every((a) => a.hotReloads) ? "hot-reload (no restart)" : "mixed"}</dd>
            </dl>
          </div>
          <div className="ec-stream ec-stream--artifact">
            <div className="ec-stream__head">
              <Tag type="purple">artifact</Tag><strong>{ev.streamTags.artifact}</strong>
            </div>
            <dl className="ec-kv">
              <dt>Pinned</dt><dd>{mine.length - unpinned} of {mine.length}</dd>
              <dt>Promotion</dt>
              <dd>{unpinned ? <Tag type="red">blocked — devMode</Tag> : <Tag type="green">eligible</Tag>}</dd>
            </dl>
          </div>
        </div>
      )}

      <h2>Profiles</h2>
      <div className="ec-tablewrap">
        <table className="ec-table">
          <thead><tr><th>Profile</th><th>Family</th></tr></thead>
          <tbody>{def.profiles.map((p) => (
            <tr key={p.name}><td>{p.name}</td><td><Tag type="blue">{p.family}</Tag></td></tr>
          ))}</tbody>
        </table>
      </div>
    </>
  );
}

function Card({ label, value, note }: { label: string; value: React.ReactNode; note: string }) {
  return (
    <div className="ec-card">
      <div className="ec-card__label">{label}</div>
      <div className="ec-card__value">{value}</div>
      <div className="ec-card__note">{note}</div>
    </div>
  );
}

/* ── Config ───────────────────────────────────────────────────────────────────────── */

export function Config({ def, sel }: { def: DefinitionView; sel: Selection }) {
  const t = useTopology(def);
  const scopeId = selectionScopeId(sel, def);
  if (!scopeId) return null;
  const chain = t.chainOf(scopeId);
  const node = sel.kind === "node" ? t.nodeByKey(sel.id) : undefined;
  const nodes = node ? [node] : t.nodesUnder(scopeId);
  const comps = nodes.reduce((a, n) => a + n.components.length, 0);
  const ownLayer = t.scopeById(scopeId)?.layer;

  return (
    <>
      <h2>Merge order</h2>
      <p className="ec-sub">Applied in order. Later entries win; the component leaf wins last.</p>
      <div className="ec-chain">
        {chain.map((s, i) => (
          <div key={s.id} className={`ec-chain__row${s.layer ? "" : " ec-chain__row--empty"}`}>
            <span className="ec-chain__idx">{i + 1}</span>
            <span><span className="ec-level">{levelOf(s.id)}</span> {valueOf(s.id)}</span>
            <code>{s.layer ?? "no layer authored at this scope"}</code>
          </div>
        ))}
        {node?.components.map((c, i) => (
          <div key={c.name} className="ec-chain__row ec-chain__row--leaf">
            <span className="ec-chain__idx">{chain.length + i + 1}</span>
            <span>{c.name}</span>
            <code>{c.layer ?? "no leaf authored"}</code>
          </div>
        ))}
      </div>

      <h2>Layer at this scope</h2>
      <div className="ec-writes">
        <span><strong>File</strong> <code>{ownLayer ?? "— none authored"}</code></span>
        <span><strong>Applies to</strong> {nodes.length} node(s) · {comps} component(s)</span>
      </div>

      <h2>Derived</h2>
      <p className="ec-sub">Computed from placement and merged into every component here.</p>
      <dl className="ec-kv">
        <dt>hierarchy.levels</dt><dd><code>{def.hierarchy.levels.join(", ")}</code></dd>
        {chain.map((s) => (
          <div key={s.id} style={{ display: "contents" }}>
            <dt>identity.{levelOf(s.id)}</dt><dd><code>{valueOf(s.id)}</code></dd>
          </div>
        ))}
      </dl>
    </>
  );
}

/* ── Render ───────────────────────────────────────────────────────────────────────── */

export function Render({ def, sel, profile }: { def: DefinitionView; sel: Selection; profile: string }) {
  const t = useTopology(def);
  const { data, error } = useAsync<RenderView>(() => api.render(profile), [profile]);
  if (error) return <InlineNotification kind="error" title="Render unavailable" subtitle={error} hideCloseButton />;
  if (!data) return <Loading small withOverlay={false} />;

  const scopeId = selectionScopeId(sel, def);
  const keys = new Set(
    sel.kind === "node" ? [sel.id] : scopeId ? t.nodesUnder(scopeId).map((n) => n.key) : [],
  );
  const entries = data.plan.entries.filter((e) => keys.has(e.node));
  const files = data.files.filter((f) => [...keys].some((k) => f.path.startsWith(`${k}/`)));

  return (
    <>
      <p className="ec-sub">
        Target <Tag type="blue">{data.target}</Tag> · environment <Tag type="cool-gray">{data.environment}</Tag>
      </p>
      <h2>Plan for this selection</h2>
      <p className="ec-sub">Restart impact follows the config source.</p>
      <div className="ec-tablewrap">
        <table className="ec-table">
          <thead><tr><th>Node</th><th>Component</th><th>Consequence</th><th>Restarts</th><th>Summary</th></tr></thead>
          <tbody>{entries.map((e, i) => (
            <tr key={i}>
              <td>{e.node}</td><td>{e.component}</td>
              <td><Tag type={e.consequence === "artifact" ? "purple" : "teal"}>{e.consequence}</Tag></td>
              <td>{e.restartsComponent ? "yes" : "no"}</td>
              <td>{e.summary}</td>
            </tr>
          ))}</tbody>
        </table>
      </div>

      <h2>Rendered files ({files.length})</h2>
      {files.map((f) => (
        <details key={f.path} className="ec-file">
          <summary>{f.path}</summary>
          <pre>{f.text}</pre>
        </details>
      ))}
    </>
  );
}

/* ── Releases — the gate: two streams, approvals, evidence ────────────────────────── */

export function Releases({ profile }: { profile: string }) {
  const { data: ev, error: evErr } = useAsync<EvidenceView>(() => api.evidence(profile), [profile]);
  const { data: ac } = useAsync<AccessView>(() => api.access(), []);

  if (evErr) return <InlineNotification kind="error" title="Evidence unavailable" subtitle={evErr} hideCloseButton />;
  if (!ev) return <Loading small withOverlay={false} />;
  const m = ev.manifest;

  return (
    <>
      <h2>Pending gate</h2>
      <p className="ec-sub">Each stream promotes and rolls back on its own.</p>
      <div className="ec-streams">
        {(["config", "artifact"] as const).map((s) => (
          <div key={s} className={`ec-stream ec-stream--${s}`}>
            <div className="ec-stream__head">
              <Tag type={s === "config" ? "teal" : "purple"}>{s}</Tag>
              <strong>{ev.streamTags[s]}</strong>
            </div>
            <dl className="ec-kv">
              <dt>Definition commit</dt><dd><code>{m.definitionCommit}</code></dd>
              <dt>Files</dt><dd>{m.files.length}</dd>
              <dt>Release hash</dt><dd><code>{m.releaseHash.slice(0, 26)}…</code></dd>
              <dt>devMode</dt><dd>{m.devMode ? <Tag type="red">yes</Tag> : "no"}</dd>
            </dl>
            {s === "artifact" && m.devMode && (
              <InlineNotification kind="warning" lowContrast hideCloseButton title="Promotion blocked"
                subtitle="At least one artifact is source-form. A protected environment requires version + digest — run deployment lock." />
            )}
          </div>
        ))}
      </div>

      <h2>Approvals</h2>
      {!ac ? <Loading small withOverlay={false} /> : ac.codeowners ? (
        <>
          <p className="ec-sub">Required reviewers, from <code>{ac.codeowners.path}</code>.</p>
          <div className="ec-tablewrap">
            <table className="ec-table">
              <thead><tr><th>Scope</th><th>Component</th><th>File</th><th>Required reviewers</th></tr></thead>
              <tbody>
                <tr>
                  <td>definition</td><td>—</td>
                  <td><code>{ac.definitionFile.file}</code></td>
                  <td>{ac.definitionFile.owners.map((o) => <Tag key={o} type="teal">{o}</Tag>)}</td>
                </tr>
                {ac.items.map((it, i) => (
                  <tr key={i}>
                    <td>{it.scope ?? "—"}</td><td>{it.component ?? "—"}</td>
                    <td><code>{it.file}</code></td>
                    <td>{it.owners.length
                      ? it.owners.map((o) => <Tag key={o} type="teal">{o}</Tag>)
                      : <Tag type="cool-gray">default branch protection</Tag>}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      ) : (
        <InlineNotification kind="warning" lowContrast hideCloseButton title="No CODEOWNERS"
          subtitle={ac.note} />
      )}

      <h2>Evidence bundle</h2>
      <dl className="ec-kv">
        <dt>Schema validation</dt><dd>{ev.evidence.schemaValidation}</dd>
        <dt>Semantic rules</dt><dd>{ev.evidence.semanticRules}</dd>
        <dt>Render determinism</dt><dd>{ev.evidence.renderDeterminism}</dd>
        <dt>Warnings</dt>
        <dd>{ev.evidence.warnings.length ? ev.evidence.warnings.join("; ") : "none"}</dd>
      </dl>
    </>
  );
}
