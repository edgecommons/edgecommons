import { useEffect, useState } from "react";
import {
  Table, TableHead, TableRow, TableHeader, TableBody, TableCell,
  Tag, Loading, InlineNotification,
} from "@carbon/react";
import { api, type AccessView, type AccessItem } from "./api";

// The access-control screen: a rendering of the repository's CODEOWNERS (REVIEW #10). It reports who
// a change to each deployment file would require as a reviewer — it never invents an approval lane,
// only surfaces the Git-host rule that already governs the file. No CODEOWNERS means the honest
// "falls to default branch-protection review", never "unrestricted".
export function AccessControl() {
  const [data, setData] = useState<AccessView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    api.access().then(setData).catch((e: Error) => setError(e.message));
  }, []);
  if (error) return <InlineNotification kind="error" title="Access data unavailable" subtitle={error} hideCloseButton />;
  if (!data) return <Loading small withOverlay={false} />;

  return (
    <div style={{ marginTop: "1rem" }}>
      <p>
        {data.codeowners
          ? <>Ownership rendered from <Tag type="blue">{data.codeowners.path}</Tag></>
          : <Tag type="red">no CODEOWNERS</Tag>}
      </p>
      <InlineNotification
        kind={data.codeowners ? (data.unownedCount ? "warning" : "info") : "warning"}
        lowContrast
        hideCloseButton
        title="Review requirement"
        subtitle={data.note}
      />
      <div style={{ marginTop: "1rem" }}>
      <Table size="sm">
        <TableHead>
          <TableRow>
            <TableHeader>Scope</TableHeader>
            <TableHeader>Component</TableHeader>
            <TableHeader>File</TableHeader>
            <TableHeader>Required reviewers</TableHeader>
            <TableHeader>Rule</TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          <Line key="definition" item={data.definitionFile} scopeLabel="definition" componentLabel="—" />
          {data.items.map((it, i) => (
            <Line key={i} item={it} scopeLabel={it.scope ?? "—"} componentLabel={it.component ?? "—"} />
          ))}
        </TableBody>
      </Table>
      </div>
    </div>
  );
}

function Line({ item, scopeLabel, componentLabel }: { item: AccessItem; scopeLabel: string; componentLabel: string }) {
  const owned = item.owners.length > 0;
  return (
    <TableRow>
      <TableCell>{scopeLabel}</TableCell>
      <TableCell>{componentLabel}</TableCell>
      <TableCell><code style={{ fontSize: "0.75rem" }}>{item.file}</code></TableCell>
      <TableCell>
        {owned
          ? item.owners.map((o) => <Tag key={o} type="teal">{o}</Tag>)
          : <Tag type="cool-gray">default branch protection</Tag>}
      </TableCell>
      <TableCell>{item.matchedPattern ? <code style={{ fontSize: "0.75rem" }}>{item.matchedPattern}</code> : "—"}</TableCell>
    </TableRow>
  );
}
