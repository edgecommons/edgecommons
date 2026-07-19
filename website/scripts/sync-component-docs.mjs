#!/usr/bin/env node
/**
 * sync-component-docs.mjs
 *
 * Aggregates each registered component's `docs/` into the Starlight site at
 * `src/content/docs/components/<name>/`, so the one EdgeCommons docs site carries both the
 * `edgecommons` library docs and every component's user/deployer guide.
 *
 * - Component list comes from the LIVE registry (single source of truth, also read by the org
 *   profile): a shallow `git clone` of edgecommons/registry at build time. Override the source
 *   with REGISTRY_JSON (a local path) or REGISTRY_REPO (a different repo); if the clone fails it
 *   falls back to the in-repo staged copy so the site still builds offline.
 * - Each component's docs come from either:
 *     dev: a local path map in $COMPONENT_DOCS_MAP (JSON: {"opcua-adapter":"/abs/path", ...})
 *     CI:  a shallow, sparse `git clone` of the component repo using $EDGECOMMONS_READ_TOKEN.
 * - NON-FATAL: if a component's docs can't be obtained, it's skipped with a warning and the site
 *   still builds (so the live docs never break just because a token isn't configured yet).
 *
 * Component docs stay PLAIN markdown in their own repos; this script injects Starlight frontmatter
 * (title from the first H1; sidebar order from the Diátaxis filename), rewrites relative `.md`
 * cross-links to Starlight routes, and flattens the `reference/` subdir for clean sidebar ordering.
 */
import { readFileSync, writeFileSync, mkdirSync, rmSync, existsSync, readdirSync, statSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB = join(__dirname, "..");
const REPO = join(WEB, "..");
const OUT = join(WEB, "src/content/docs/components");
const TMP = join(WEB, ".component-src");
const TOKEN = process.env.EDGECOMMONS_READ_TOKEN || "";
const LOCAL_MAP = JSON.parse(process.env.COMPONENT_DOCS_MAP || "{}");
const REGISTRY_REPO = process.env.REGISTRY_REPO || "edgecommons/registry";

// The component list is the LIVE registry — the same repo the org profile reads, so registering a
// component in edgecommons/registry updates both surfaces with no staged-copy sync. REGISTRY_JSON
// overrides with a local path (dev/CI); a clone failure falls back to the in-repo staged copy.
function resolveRegistry() {
  if (process.env.REGISTRY_JSON) return process.env.REGISTRY_JSON;
  const dst = join(TMP, "_registry");
  rmSync(dst, { recursive: true, force: true });
  mkdirSync(dst, { recursive: true });
  const url = TOKEN
    ? `https://x-access-token:${TOKEN}@github.com/${REGISTRY_REPO}.git`
    : `https://github.com/${REGISTRY_REPO}.git`;
  try {
    execFileSync("git", ["clone", "--depth", "1", url, dst], { stdio: "pipe" });
    return join(dst, "components.json");
  } catch (e) {
    console.warn(`! could not clone ${REGISTRY_REPO}; using the staged copy: ${String(e.message).split("\n")[0]}`);
    return join(REPO, "ecosystem/staging/registry/components.json");
  }
}
const REGISTRY = resolveRegistry();

// Sidebar order by source filename (Diátaxis). reference/* and deployment/* are flattened to
// reference-<x> (30+) and deployment-<x> (45+).
const ORDER = { index: 0, tutorial: 10, "user-guide": 15, "how-to-guides": 20, scripting: 22, "sample-configurations": 25, explanation: 40 };

function jsonStr(s) {
  return JSON.stringify(String(s));
}

function normalizeSegs(segs) {
  const out = [];
  for (const s of segs) {
    if (s === "" || s === ".") continue;
    if (s === "..") {
      if (out.length && out[out.length - 1] !== "..") out.pop();
      else out.push("..");
    } else out.push(s);
  }
  return out;
}

// Resolve one relative doc link to a site route, resolving it relative to the source file's
// location within docs/ (so within-reference/ siblings flatten to reference-<x>, and links that
// escape docs/ become GitHub repo URLs). Returns null to leave the link unchanged.
function resolveDocLink(target, { name, repo, fileDir }) {
  const h = target.indexOf("#");
  const anchor = h >= 0 ? target.slice(h) : "";
  const path = h >= 0 ? target.slice(0, h) : target;
  const segs = normalizeSegs([...(fileDir ? fileDir.split("/") : []), ...path.split("/")]);
  if (segs[0] === "..") {
    // escapes docs/ (docs is one level under the repo root) -> a repo file on GitHub
    return repo ? `https://github.com/${repo}/blob/main/${segs.slice(1).join("/")}${anchor}` : null;
  }
  const last = segs.length ? segs[segs.length - 1] : "";
  if (!/\.mdx?$/i.test(last)) {
    // a directory link (reference/, the docs root, …)
    if (segs.includes("reference")) return `/components/${name}/reference-configuration/${anchor}`;
    if (segs.length === 0) return `/components/${name}/${anchor}`;
    return null; // unknown non-.md relative link — leave as-is
  }
  const baseName = last.replace(/\.mdx?$/i, "");
  if (/^(readme|index)$/i.test(baseName)) return `/components/${name}/${anchor}`;
  if (segs.includes("reference")) return `/components/${name}/reference-${baseName}/${anchor}`;
  if (segs.includes("deployment")) return `/components/${name}/deployment-${baseName}/${anchor}`;
  return `/components/${name}/${baseName}/${anchor}`;
}

function rewriteLinks(body, opts) {
  return body.replace(/\]\(([^)\s]+)\)/g, (m, target) => {
    if (/^(https?:|\/|#|mailto:|tel:|data:)/i.test(target)) return m; // absolute / anchor / external
    const r = resolveDocLink(target, opts);
    return r ? `](${r})` : m;
  });
}

function toStarlight(raw, { title, description, order, name, repo, fileDir, isMdx }) {
  let body = raw;
  const h1 = raw.match(/^\s*#\s+(.+?)\s*$/m);
  if (!title) title = h1 ? h1[1].replace(/`/g, "") : "Untitled";
  if (h1) body = raw.slice(0, h1.index) + raw.slice(h1.index + h1[0].length).replace(/^\n+/, "\n");
  body = rewriteLinks(body, { name, repo, fileDir }).replace(/^\s+/, "");
  let fm = `---\ntitle: ${jsonStr(title)}\n`;
  if (description) fm += `description: ${jsonStr(description)}\n`;
  fm += `sidebar:\n  order: ${order}\n---\n\n`;
  // An .mdx component doc may use Starlight components (<Tabs>, <Aside>, …) without importing
  // them (component docs stay renderer-agnostic in their repos) — inject the standard import.
  if (isMdx && !/from\s+["']@astrojs\/starlight\/components["']/.test(body)) {
    const used = [...new Set(body.match(/<(Tabs|TabItem|Aside|Steps|Card|CardGrid|LinkCard|Badge|Code|FileTree|Icon)[\s>]/g) || [])]
      .map((m) => m.replace(/[<\s>]/g, ""));
    if (used.length) fm += `import { ${used.join(", ")} } from "@astrojs/starlight/components";\n\n`;
  }
  return fm + body;
}

function obtainDocs(c) {
  if (LOCAL_MAP[c.name]) {
    const p = LOCAL_MAP[c.name];
    if (existsSync(join(p, "docs"))) return join(p, "docs");
    if (existsSync(p)) return p;
    console.warn(`! local path for ${c.name} not found: ${p}`);
    return null;
  }
  const dst = join(TMP, c.name);
  rmSync(dst, { recursive: true, force: true });
  mkdirSync(dst, { recursive: true });
  const url = TOKEN
    ? `https://x-access-token:${TOKEN}@github.com/${c.repo}.git`
    : `https://github.com/${c.repo}.git`;
  try {
    // execFileSync (no shell): repo/token are passed as args, not interpolated into a shell string.
    execFileSync("git", ["clone", "--depth", "1", "--filter=blob:none", "--sparse", url, dst], { stdio: "pipe" });
    execFileSync("git", ["-C", dst, "sparse-checkout", "set", "docs"], { stdio: "pipe" });
  } catch (e) {
    console.warn(`! could not clone ${c.repo} (token set: ${Boolean(TOKEN)}): ${String(e.message).split("\n")[0]}`);
    return null;
  }
  return existsSync(join(dst, "docs")) ? join(dst, "docs") : null;
}

function syncComponent(c) {
  const docsDir = obtainDocs(c);
  if (!docsDir) return false;
  const dest = join(OUT, c.name);
  mkdirSync(dest, { recursive: true });
  for (const entry of readdirSync(docsDir)) {
    const src = join(docsDir, entry);
    if (statSync(src).isDirectory()) {
      if (entry !== "reference" && entry !== "deployment") continue; // known Diátaxis subdirs
      const [label, base] = entry === "reference" ? ["Reference", 30] : ["Deployment", 45];
      let i = 0;
      for (const ref of readdirSync(src).filter((f) => /\.mdx?$/i.test(f)).sort()) {
        const ext = /\.mdx$/i.test(ref) ? ".mdx" : ".md";
        const name = ref.replace(/\.mdx?$/i, "");
        const md = toStarlight(readFileSync(join(src, ref), "utf8"), {
          title: `${label} — ${titleCase(name)}`,
          order: base + i++,
          name: c.name,
          repo: c.repo,
          fileDir: entry,
          isMdx: ext === ".mdx",
        });
        writeFileSync(join(dest, `${entry}-${name}${ext}`), md);
      }
      continue;
    }
    if (!/\.mdx?$/i.test(entry)) continue;
    const ext = /\.mdx$/i.test(entry) ? ".mdx" : ".md";
    const isIndex = entry.toLowerCase() === "readme.md";
    const slug = isIndex ? "index" : entry.replace(/\.mdx?$/i, "");
    const md = toStarlight(readFileSync(src, "utf8"), {
      title: isIndex ? c.name : undefined,
      description: isIndex ? c.description : undefined,
      order: ORDER[slug] ?? 50,
      name: c.name,
      repo: c.repo,
      fileDir: "",
      isMdx: ext === ".mdx",
    });
    writeFileSync(join(dest, `${slug}${ext}`), md);
  }
  return true;
}

function titleCase(s) {
  return s.replace(/[-_]/g, " ").replace(/\b\w/g, (m) => m.toUpperCase());
}

function writeComponentsLanding(synced) {
  const rows = synced
    .map(
      (c) =>
        `| [${c.name}](/components/${c.name}/) | ${c.language || "—"} | ${c.protocol || c.category || "—"} | ${(c.platforms || []).join(" · ") || "—"} |`,
    )
    .join("\n");
  const md = `---
title: Components
description: User and deployer guides for the components in the EdgeCommons ecosystem.
sidebar:
  order: 0
---

The EdgeCommons ecosystem ships ready-to-deploy components built on the \`edgecommons\` library —
protocol **adapters**, edge **processors**, and northbound **sinks**. Each component's operator /
integrator guide lives below; scaffold your own with \`edgecommons create-component\`.

| Component | Language | Protocol / Category | Platforms |
|-----------|----------|---------------------|-----------|
${rows || "| _none yet_ | | | |"}
`;
  mkdirSync(OUT, { recursive: true });
  writeFileSync(join(OUT, "index.md"), md);
}

// --- main ---
const registry = JSON.parse(readFileSync(REGISTRY, "utf8"));
rmSync(OUT, { recursive: true, force: true });
mkdirSync(OUT, { recursive: true });
const synced = [];
for (const c of registry.components || []) {
  if (syncComponent(c)) {
    synced.push(c);
    console.log(`✓ synced ${c.name}`);
  } else {
    console.warn(`- skipped ${c.name} (docs unavailable)`);
  }
}
writeComponentsLanding(synced);
console.log(`component docs sync complete: ${synced.length}/${(registry.components || []).length} component(s).`);
