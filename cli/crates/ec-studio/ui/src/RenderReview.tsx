import { useEffect, useState } from "react";
import {
  Table, TableHead, TableRow, TableHeader, TableBody, TableCell,
  CodeSnippet, Tag, Loading, InlineNotification,
} from "@carbon/react";
import { api, type RenderView } from "./api";

export function RenderReview({ profile }: { profile: string }) {
  const [data, setData] = useState<RenderView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setData(null); setError(null);
    api.render(profile).then(setData).catch((e: Error) => setError(e.message));
  }, [profile]);
  if (error) return <InlineNotification kind="error" title="Render unavailable" subtitle={error} hideCloseButton />;
  if (!data) return <Loading small withOverlay={false} />;
  return (
    <div style={{ marginTop: "1rem" }}>
      <p>
        Target <Tag type="blue">{data.target}</Tag> · environment <Tag type="cool-gray">{data.environment}</Tag>
        {" "}· {data.files.length} files
      </p>
      <h3 style={{ fontWeight: 400 }}>Plan ({data.plan.entries.length} entries)</h3>
      <Table size="sm">
        <TableHead>
          <TableRow>
            <TableHeader>Node</TableHeader><TableHeader>Component</TableHeader>
            <TableHeader>Consequence</TableHeader><TableHeader>Restart</TableHeader>
            <TableHeader>Summary</TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          {data.plan.entries.map((e, i) => (
            <TableRow key={i}>
              <TableCell>{e.node}</TableCell>
              <TableCell>{e.component}</TableCell>
              <TableCell><Tag type={e.consequence === "artifact" ? "purple" : "teal"}>{e.consequence}</Tag></TableCell>
              <TableCell>{e.restartsComponent ? "yes" : "no"}</TableCell>
              <TableCell>{e.summary}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
      <h3 style={{ fontWeight: 400, marginTop: "2rem" }}>Rendered artifacts</h3>
      {data.files.map((f) => (
        <details key={f.path} style={{ margin: "0.5rem 0" }}>
          <summary style={{ cursor: "pointer", padding: "0.5rem 0" }}>{f.path}</summary>
          <CodeSnippet type="multi" feedback="Copied">{f.text}</CodeSnippet>
        </details>
      ))}
    </div>
  );
}
