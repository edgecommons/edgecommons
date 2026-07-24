// The wire contract between the Studio server and this UI — mirrors ec-studio/src/lib.rs.
export interface Profile { name: string; family: string; }
export interface NodeView { key: string; scope: string; components: string[]; }
export interface DefinitionView {
  name: string;
  description: string | null;
  profiles: Profile[];
  nodes: NodeView[];
}
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
};
