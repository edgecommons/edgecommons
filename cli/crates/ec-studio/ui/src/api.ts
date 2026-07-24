// The wire contract between the Studio server and this UI — mirrors ec-studio/src/lib.rs.
export interface Profile { name: string; family: string; }
export interface ScopeView { id: string; parent: string | null; layer: string | null; }
export interface HierarchyView { levels: string[]; scopes: ScopeView[]; }
export interface ComponentView { name: string; layer: string | null; }
export interface NodeView { key: string; scope: string; components: ComponentView[]; }
export interface DefinitionView {
  name: string;
  description: string | null;
  profiles: Profile[];
  hierarchy: HierarchyView;
  nodes: NodeView[];
}

/** A scope id is `<level>/<value>`; the level vocabulary is the customer's, never ours. */
export const levelOf = (scopeId: string) => scopeId.split("/")[0];
export const valueOf = (scopeId: string) => scopeId.split("/").slice(1).join("/");
export interface LayerComponent { node: string; component: string; config: unknown; }
export interface LayerEnv { environment: string; components: LayerComponent[]; }
export interface LayersView { profile: string; environments: LayerEnv[]; }
export interface RenderedFile { path: string; text: string; }
export interface PlanEntry {
  node: string;
  component: string;
  consequence: string;
  summary: string;
  restartsComponent: boolean;
}
export interface RenderView {
  profile: string;
  target: string;
  environment: string;
  files: RenderedFile[];
  plan: { entries: PlanEntry[] };
}

// Evidence correlation — the release lock a profile would produce (REVIEW #13).
export interface ArtifactStreamEntry {
  node: string;
  component: string;
  version: string | null;
  digest: string | null;
  configSource: string;
  hotReloads: boolean;
}
export interface ManifestFile { path: string; sha256: string; }
export interface EvidenceManifest {
  release: string;
  devMode: boolean;
  releaseHash: string;
  definitionCommit: string;
  renderer: string;
  streams: {
    config: Record<string, { catalogVersion?: string | null; catalogSha256?: string | null; bootstrapSha256?: string | null }>;
    artifact: ArtifactStreamEntry[];
  };
  files: ManifestFile[];
}
export interface EvidenceBundle {
  schemaValidation: string;
  semanticRules: string;
  warnings: string[];
  renderDeterminism: string;
}
export interface EvidenceView {
  profile: string;
  target: string;
  environment: string;
  commit: string;
  streamTags: { config: string; artifact: string };
  manifest: EvidenceManifest;
  evidence: EvidenceBundle;
}

// Access control — a rendering of the repo's CODEOWNERS (REVIEW #10).
export interface AccessItem {
  file: string;
  owners: string[];
  matchedPattern: string | null;
  node?: string;
  scope?: string;
  component?: string;
}
export interface AccessView {
  codeowners: { path: string } | null;
  unownedCount: number;
  note: string;
  definitionFile: AccessItem;
  items: AccessItem[];
}

async function get<T>(url: string): Promise<T> {
  const r = await fetch(url);
  if (!r.ok) {
    const body = (await r.json().catch(() => ({}))) as { error?: string };
    throw new Error(body.error ?? `request failed (${r.status})`);
  }
  return (await r.json()) as T;
}

export const api = {
  definition: () => get<DefinitionView>("/api/definition"),
  layers: (profile: string) => get<LayersView>(`/api/profiles/${profile}/layers`),
  render: (profile: string) => get<RenderView>(`/api/profiles/${profile}/render`),
  evidence: (profile: string) => get<EvidenceView>(`/api/profiles/${profile}/evidence`),
  access: () => get<AccessView>("/api/access"),
};
