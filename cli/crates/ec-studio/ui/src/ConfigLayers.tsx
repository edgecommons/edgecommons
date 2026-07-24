import { useEffect, useState } from "react";
import { CodeSnippet, Loading, InlineNotification } from "@carbon/react";
import { api, type LayersView } from "./api";

export function ConfigLayers({ profile }: { profile: string }) {
  const [data, setData] = useState<LayersView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setData(null); setError(null);
    api.layers(profile).then(setData).catch((e: Error) => setError(e.message));
  }, [profile]);
  if (error) return <InlineNotification kind="error" title="Config layers unavailable" subtitle={error} hideCloseButton />;
  if (!data) return <Loading small withOverlay={false} />;
  return (
    <div style={{ marginTop: "1rem" }}>
      {data.environments.map((env) => (
        <section key={env.environment} style={{ marginBottom: "2rem" }}>
          <h3 style={{ fontWeight: 400 }}>Environment: {env.environment}</h3>
          {env.components.map((c) => (
            <details key={`${c.node}/${c.component}`} style={{ margin: "0.5rem 0" }}>
              <summary style={{ cursor: "pointer", padding: "0.5rem 0" }}>
                <strong>{c.node}</strong> / {c.component}
              </summary>
              <CodeSnippet type="multi" feedback="Copied">{JSON.stringify(c.config, null, 2)}</CodeSnippet>
            </details>
          ))}
        </section>
      ))}
    </div>
  );
}
