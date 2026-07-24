import { useEffect, useState } from "react";
import {
  Table, TableHead, TableRow, TableHeader, TableBody, TableCell,
  Tag, Loading, InlineNotification, StructuredListWrapper, StructuredListHead,
  StructuredListRow, StructuredListCell, StructuredListBody,
} from "@carbon/react";
import { api, type EvidenceView } from "./api";

// The evidence-correlation screen: the release lock a profile would produce — the two streams
// correlated but never fused, per-file digests, and the evidence bundle. The Studio holds intent
// and adjudicates delivery from evidence (REVIEW #13); it computes this envelope and writes nothing.
export function EvidenceReview({ profile }: { profile: string }) {
  const [data, setData] = useState<EvidenceView | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    setData(null); setError(null);
    api.evidence(profile).then(setData).catch((e: Error) => setError(e.message));
  }, [profile]);
  if (error) return <InlineNotification kind="error" title="Evidence unavailable" subtitle={error} hideCloseButton />;
  if (!data) return <Loading small withOverlay={false} />;

  const m = data.manifest;
  const dirty = data.commit.endsWith("-dirty") || data.commit === "unknown";
  return (
    <div style={{ marginTop: "1rem" }}>
      <p>
        Target <Tag type="blue">{data.target}</Tag> · environment <Tag type="cool-gray">{data.environment}</Tag>
        {" "}· definition commit{" "}
        <Tag type={dirty ? "red" : "green"}>{data.commit}</Tag>
      </p>
      {m.devMode && (
        <InlineNotification
          kind="warning"
          lowContrast
          hideCloseButton
          title="devMode"
          subtitle="At least one artifact is source-form (not version+digest pinned). Promotion to a protected environment requires full pins — run deployment lock."
        />
      )}

      <h3 style={{ fontWeight: 400 }}>Two streams, correlated — never fused</h3>
      <StructuredListWrapper aria-label="release streams" isCondensed>
        <StructuredListHead>
          <StructuredListRow head>
            <StructuredListCell head>Stream</StructuredListCell>
            <StructuredListCell head>Release tag</StructuredListCell>
            <StructuredListCell head>Independently rolled back</StructuredListCell>
          </StructuredListRow>
        </StructuredListHead>
        <StructuredListBody>
          <StructuredListRow>
            <StructuredListCell><Tag type="teal">config</Tag></StructuredListCell>
            <StructuredListCell>{data.streamTags.config}</StructuredListCell>
            <StructuredListCell>yes — promoting one never moves the other</StructuredListCell>
          </StructuredListRow>
          <StructuredListRow>
            <StructuredListCell><Tag type="purple">artifact</Tag></StructuredListCell>
            <StructuredListCell>{data.streamTags.artifact}</StructuredListCell>
            <StructuredListCell>yes — promoting one never moves the other</StructuredListCell>
          </StructuredListRow>
        </StructuredListBody>
      </StructuredListWrapper>

      <h3 style={{ fontWeight: 400, marginTop: "2rem" }}>Artifact stream</h3>
      <Table size="sm">
        <TableHead>
          <TableRow>
            <TableHeader>Node</TableHeader><TableHeader>Component</TableHeader>
            <TableHeader>Version</TableHeader><TableHeader>Config source</TableHeader>
            <TableHeader>Hot-reloads</TableHeader>
          </TableRow>
        </TableHead>
        <TableBody>
          {m.streams.artifact.map((a, i) => (
            <TableRow key={i}>
              <TableCell>{a.node}</TableCell>
              <TableCell>{a.component}</TableCell>
              <TableCell>{a.version ?? <Tag type="red">source-form</Tag>}</TableCell>
              <TableCell><Tag type="teal">{a.configSource}</Tag></TableCell>
              <TableCell>{a.hotReloads ? "yes" : "no"}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>

      <h3 style={{ fontWeight: 400, marginTop: "2rem" }}>Evidence bundle</h3>
      <StructuredListWrapper aria-label="evidence bundle" isCondensed>
        <StructuredListBody>
          <Row label="Schema validation" value={data.evidence.schemaValidation} />
          <Row label="Semantic rules" value={data.evidence.semanticRules} />
          <Row label="Render determinism" value={data.evidence.renderDeterminism} />
          <Row label="Release hash" value={m.releaseHash} mono />
          <Row
            label="Warnings"
            value={data.evidence.warnings.length ? data.evidence.warnings.join("; ") : "none"}
          />
        </StructuredListBody>
      </StructuredListWrapper>

      <h3 style={{ fontWeight: 400, marginTop: "2rem" }}>Rendered files ({m.files.length})</h3>
      <Table size="sm">
        <TableHead>
          <TableRow><TableHeader>Path</TableHeader><TableHeader>sha256</TableHeader></TableRow>
        </TableHead>
        <TableBody>
          {m.files.map((f) => (
            <TableRow key={f.path}>
              <TableCell>{f.path}</TableCell>
              <TableCell><code style={{ fontSize: "0.75rem" }}>{f.sha256}</code></TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <StructuredListRow>
      <StructuredListCell>{label}</StructuredListCell>
      <StructuredListCell>
        {mono ? <code style={{ fontSize: "0.75rem" }}>{value}</code> : value}
      </StructuredListCell>
    </StructuredListRow>
  );
}
