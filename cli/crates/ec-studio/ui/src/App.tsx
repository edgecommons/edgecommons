import { useEffect, useState } from "react";
import { Theme, Loading, InlineNotification } from "@carbon/react";
import { api, levelOf, valueOf, type DefinitionView } from "./api";
import {
  Breadcrumb, Rail, selectionScopeId, tabsFor, useTopology,
  type Selection, type Tab,
} from "./Shell";
import { Config, NotBuilt, Overview, Releases, Render } from "./Panels";
// The canonical EdgeCommons horizontal lockup, reversed for the dark context bar.
import logoUrl from "./assets/edgecommons-logo-horizontal-reversed.svg";

export function App() {
  const [def, setDef] = useState<DefinitionView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [profile, setProfile] = useState("");
  const [sel, setSel] = useState<Selection | null>(null);
  const [tab, setTab] = useState<Tab>("Overview");

  useEffect(() => {
    api.definition()
      .then((d) => {
        setDef(d);
        if (d.profiles[0]) setProfile(d.profiles[0].name);
        const root = d.hierarchy.scopes.find((s) => !s.parent);
        if (root) setSel({ kind: "scope", id: root.id });
      })
      .catch((e: Error) => setError(e.message));
  }, []);

  if (error) {
    return (
      <Theme theme="g100" className="ec-app ec-app--dark">
        <main className="ec-content">
          <InlineNotification kind="error" title="Cannot reach the Studio server" subtitle={error} hideCloseButton />
        </main>
      </Theme>
    );
  }
  if (!def || !sel) return <Loading />;

  return (
    <Theme theme="g100" className="ec-app ec-app--dark ec-shell">
      <ContextBar def={def} profile={profile} onProfile={setProfile} />
      <div className="ec-body">
        <Rail def={def} sel={sel} onSelect={(s) => { setSel(s); setTab("Overview"); }} />
        <main className="ec-workspace">
          <Breadcrumb def={def} sel={sel} onSelect={(s) => { setSel(s); setTab("Overview"); }} />
          <SelectionHeader def={def} sel={sel} />
          <Tabs def={def} sel={sel} tab={tab} onTab={setTab} />
          <section className="ec-panel">
            <Panel def={def} sel={sel} tab={tab} profile={profile} />
          </section>
        </main>
      </div>
    </Theme>
  );
}

function ContextBar({
  def, profile, onProfile,
}: { def: DefinitionView; profile: string; onProfile: (p: string) => void }) {
  return (
    <header className="ec-contextbar">
      <a className="ec-brand" href="#" aria-label="EdgeCommons"><img src={logoUrl} alt="EdgeCommons" /></a>
      <span className="ec-product">Deployment Studio</span>
      <div className="ec-contextbar__group">
        <label className="ec-field">
          <span>Workspace</span>
          <strong>{def.name}</strong>
        </label>
        <label className="ec-field">
          <span>Profile</span>
          <select className="ec-select" value={profile} onChange={(e) => onProfile(e.target.value)}>
            {def.profiles.map((p) => <option key={p.name} value={p.name}>{p.name} — {p.family}</option>)}
          </select>
        </label>
        <span className="ec-chip" title="This server serves committed state; authoring is not built">read-only</span>
      </div>
    </header>
  );
}

function SelectionHeader({ def, sel }: { def: DefinitionView; sel: Selection }) {
  const t = useTopology(def);
  if (sel.kind === "global") {
    return <header className="ec-selection"><h1>{sel.id}</h1><span className="ec-sub">global area</span></header>;
  }
  if (sel.kind === "node") {
    const n = t.nodeByKey(sel.id);
    return (
      <header className="ec-selection">
        <h1>{sel.id}</h1>
        <span className="ec-level">{t.deviceLevel}</span>
        <span className="ec-sub">{n?.components.length ?? 0} components · attached to <code>{n?.scope}</code></span>
      </header>
    );
  }
  const under = t.nodesUnder(sel.id).length;
  const children = t.subtreeScopeIds(sel.id).length - 1;
  return (
    <header className="ec-selection">
      <h1>{valueOf(sel.id)}</h1>
      <span className="ec-level">{levelOf(sel.id)}</span>
      <span className="ec-sub">{under} nodes · {children} child scopes</span>
    </header>
  );
}

function Tabs({
  def, sel, tab, onTab,
}: { def: DefinitionView; sel: Selection; tab: Tab; onTab: (t: Tab) => void }) {
  const tabs = tabsFor(sel, def);
  if (!tabs.length) return null;
  return (
    <nav className="ec-tabs" role="tablist" aria-label="Views over the current selection">
      {tabs.map((t) => (
        <button key={t} role="tab" aria-selected={t === tab} onClick={() => onTab(t)}>{t}</button>
      ))}
    </nav>
  );
}

function Panel({
  def, sel, tab, profile,
}: { def: DefinitionView; sel: Selection; tab: Tab; profile: string }) {
  if (sel.kind === "global") {
    if (sel.id === "Releases") return <Releases profile={profile} />;
    return <NotBuilt what={sel.id} detail="Not built yet. The read-only cuts cover Overview, Config, Render and the Releases gate." />;
  }
  if (!selectionScopeId(sel, def)) return null;
  switch (tab) {
    case "Overview": return <Overview def={def} sel={sel} profile={profile} />;
    case "Config": return <Config def={def} sel={sel} />;
    case "Render": return <Render def={def} sel={sel} profile={profile} />;
    case "Components": return <NotBuilt what="Components" detail="The node-anchored component editor is part of the write path, which is not built." />;
    case "Topology": return <NotBuilt what="Topology" detail="The derived read-only topology graph is not built." />;
    case "History": return <NotBuilt what="History" detail="Git history for this selection needs a log port the kernel does not expose." />;
    default: return null;
  }
}
