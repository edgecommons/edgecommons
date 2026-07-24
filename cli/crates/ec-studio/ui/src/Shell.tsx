import { useMemo } from "react";
import { levelOf, valueOf, type DefinitionView, type NodeView, type ScopeView } from "./api";

/* The context spine (REVIEW-UI §4, decision 1A): the fleet tree is a persistent selection rail whose
 * selection drives the workspace; Releases/Operations/Registry/Settings are the only global areas;
 * the breadcrumb is always present; workspace tabs appear by level.
 *
 * No level name appears anywhere below — levels come from the definition's own hierarchy, and a
 * scope's level is the part of its id before the slash. */

export type Selection =
  | { kind: "scope"; id: string }
  | { kind: "node"; id: string }
  | { kind: "global"; id: GlobalArea };

export type GlobalArea = "Releases" | "Operations" | "Registry" | "Settings";

export const GLOBAL_AREAS: { id: GlobalArea; note: string }[] = [
  { id: "Releases", note: "history + pending gates" },
  { id: "Operations", note: "drift · rollouts · evidence" },
  { id: "Registry", note: "component catalog" },
  { id: "Settings", note: "storage · profiles · approvers" },
];

/** Helpers over the hierarchy — all derived, none level-aware. */
export function useTopology(def: DefinitionView) {
  return useMemo(() => {
    const scopeById = (id: string) => def.hierarchy.scopes.find((s) => s.id === id);
    const childScopes = (id: string | null) => def.hierarchy.scopes.filter((s) => s.parent === id);
    const nodesDirectlyIn = (id: string) => def.nodes.filter((n) => n.scope === id);

    const chainOf = (scopeId: string): ScopeView[] => {
      const chain: ScopeView[] = [];
      let cur: string | null = scopeId;
      while (cur) {
        const s = scopeById(cur);
        if (!s) break;
        chain.push(s);
        cur = s.parent;
      }
      return chain.reverse();
    };

    const subtreeScopeIds = (id: string): string[] => {
      const out: string[] = [];
      const walk = (sid: string) => { out.push(sid); childScopes(sid).forEach((c) => walk(c.id)); };
      walk(id);
      return out;
    };

    const nodesUnder = (id: string): NodeView[] => {
      const ids = new Set(subtreeScopeIds(id));
      return def.nodes.filter((n) => ids.has(n.scope));
    };

    const nodeByKey = (k: string) => def.nodes.find((n) => n.key === k);
    const roots = def.hierarchy.scopes.filter((s) => !s.parent);
    const deviceLevel = def.hierarchy.levels[def.hierarchy.levels.length - 1];

    return { scopeById, childScopes, nodesDirectlyIn, chainOf, subtreeScopeIds, nodesUnder, nodeByKey, roots, deviceLevel };
  }, [def]);
}

/** The scope a selection lives at — a node resolves to the scope it is attached to. */
export function selectionScopeId(sel: Selection, def: DefinitionView): string | null {
  if (sel.kind === "global") return null;
  if (sel.kind === "node") return def.nodes.find((n) => n.key === sel.id)?.scope ?? null;
  return sel.id;
}

export const TABS = ["Overview", "Components", "Config", "Topology", "Render", "History"] as const;
export type Tab = (typeof TABS)[number];

/** Tabs by level: Overview/Config/Render/History everywhere; Components/Topology where the
 *  selection is, or directly contains, nodes. Expressed structurally, never by level name. */
export function tabsFor(sel: Selection, def: DefinitionView): Tab[] {
  if (sel.kind === "global") return [];
  const hasNodes = sel.kind === "node" || def.nodes.some((n) => n.scope === sel.id);
  return TABS.filter((t) => (t === "Components" || t === "Topology" ? hasNodes : true));
}

export function Rail({
  def, sel, onSelect,
}: { def: DefinitionView; sel: Selection; onSelect: (s: Selection) => void }) {
  const t = useTopology(def);

  const rows: JSX.Element[] = [];
  const walk = (scope: ScopeView, depth: number) => {
    const under = t.nodesUnder(scope.id).length;
    const selected = sel.kind === "scope" && sel.id === scope.id;
    rows.push(
      <button
        key={`s:${scope.id}`}
        className="ec-tree__row"
        role="treeitem"
        aria-selected={selected}
        style={{ paddingLeft: `${0.5 + depth * 0.85}rem` }}
        onClick={() => onSelect({ kind: "scope", id: scope.id })}
      >
        <span className="ec-level">{levelOf(scope.id)}</span>
        <span className="ec-tree__label">{valueOf(scope.id)}</span>
        <span className="ec-tree__count" title={`${under} node(s) beneath`}>{under}</span>
      </button>,
    );
    t.childScopes(scope.id).forEach((c) => walk(c, depth + 1));
    t.nodesDirectlyIn(scope.id).forEach((n) => {
      const nsel = sel.kind === "node" && sel.id === n.key;
      rows.push(
        <button
          key={`n:${n.key}`}
          className="ec-tree__row ec-tree__row--node"
          role="treeitem"
          aria-selected={nsel}
          style={{ paddingLeft: `${0.5 + (depth + 1) * 0.85}rem` }}
          onClick={() => onSelect({ kind: "node", id: n.key })}
        >
          <span className="ec-level">{t.deviceLevel}</span>
          <span className="ec-tree__label">{n.key}</span>
          <span className="ec-tree__count">{n.components.length}</span>
        </button>,
      );
    });
  };
  t.roots.forEach((r) => walk(r, 0));

  return (
    <nav className="ec-rail" aria-label="Context spine and global areas">
      <div className="ec-rail__heading">Context</div>
      <div className="ec-tree" role="tree" aria-label="Fleet">{rows}</div>

      <div className="ec-rail__heading ec-rail__heading--global">Global</div>
      <ul className="ec-global">
        {GLOBAL_AREAS.map((g) => (
          <li key={g.id}>
            <button
              aria-current={sel.kind === "global" && sel.id === g.id}
              onClick={() => onSelect({ kind: "global", id: g.id })}
            >
              {g.id} <small>{g.note}</small>
            </button>
          </li>
        ))}
      </ul>

      <div className="ec-rail__footer">
        <span className="ec-legend"><i className="ec-dot ec-dot--config" />config stream</span>
        <span className="ec-legend"><i className="ec-dot ec-dot--artifact" />artifact stream</span>
      </div>
    </nav>
  );
}

export function Breadcrumb({
  def, sel, onSelect,
}: { def: DefinitionView; sel: Selection; onSelect: (s: Selection) => void }) {
  const t = useTopology(def);
  const parts: JSX.Element[] = [<span key="ws" className="ec-crumb__root">{def.name}</span>];

  if (sel.kind === "global") {
    parts.push(<span key="sep-g" className="sep">/</span>, <span key="g" aria-current="page">{sel.id}</span>);
  } else {
    const scopeId = selectionScopeId(sel, def);
    if (scopeId) {
      t.chainOf(scopeId).forEach((s) => {
        const isLast = sel.kind === "scope" && s.id === scopeId;
        parts.push(<span key={`sep:${s.id}`} className="sep">/</span>);
        parts.push(
          isLast
            ? <span key={s.id} aria-current="page">{valueOf(s.id)}</span>
            : <button key={s.id} onClick={() => onSelect({ kind: "scope", id: s.id })}>{valueOf(s.id)}</button>,
        );
      });
    }
    if (sel.kind === "node") {
      parts.push(<span key="sep-n" className="sep">/</span>, <span key="n" aria-current="page">{sel.id}</span>);
    }
  }
  return <nav className="ec-crumb" aria-label="Breadcrumb">{parts}</nav>;
}
