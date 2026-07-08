const colors = ["#1f8a8a", "#b35c34", "#6a5fb8", "#d59a32", "#406c9b", "#b83f4c", "#607d3b"];

const hierarchyLevels = [
  { level: "enterprise", value: "acme-industrial", contributes: { identity: { enterprise: "acme-industrial" } } },
  { level: "site", value: "dallas", contributes: { identity: { site: "dallas" }, logging: { level: "INFO" } } },
  { level: "building", value: "b17", contributes: { identity: { building: "b17" }, tags: { building: "b17" } } },
  { level: "zone", value: "packaging", contributes: { identity: { zone: "packaging" }, metricEmission: { namespace: "edge/packaging" } } },
  { level: "line", value: "line-7", contributes: { identity: { line: "line-7" }, heartbeat: { intervalSecs: 10 } } },
];

const componentLayer = {
  level: "component",
  value: "opcua-adapter",
  contributes: {
    component: {
      token: "opcua-adapter",
      global: { endpoint: "opc.tcp://10.10.7.20:4840" },
    },
  },
};

const scenarios = {
  line: {
    title: "Line deployment",
    narrative:
      "Each ancestor contributes only the scope it owns. The component leaf keeps only adapter-specific work.",
    layers: [
      { name: "enterprise", detail: "hierarchy.levels + enterprise identity", color: colors[0] },
      { name: "site", detail: "site identity + default logging", color: colors[1] },
      { name: "building", detail: "building identity + tags", color: colors[2] },
      { name: "zone", detail: "zone identity + metric namespace", color: colors[3] },
      { name: "line", detail: "line identity + heartbeat", color: colors[4] },
      { name: "component", detail: "OPC UA endpoint + instances", color: colors[5] },
    ],
    json: {
      hierarchy: { levels: ["enterprise", "site", "building", "zone", "line", "device"] },
      identity: {
        enterprise: "acme-industrial",
        site: "dallas",
        building: "b17",
        zone: "packaging",
        line: "line-7",
      },
      logging: { level: "INFO" },
      metricEmission: { namespace: "edge/packaging" },
      heartbeat: { intervalSecs: 10 },
      component: {
        token: "opcua-adapter",
        global: { endpoint: "opc.tcp://10.10.7.20:4840" },
      },
    },
  },
  zone: {
    title: "Zone override",
    narrative:
      "A lower layer can override operational defaults without redefining the enterprise or site context.",
    layers: [
      { name: "enterprise", detail: "identity.enterprise = acme-industrial", color: colors[0] },
      { name: "site", detail: "identity.site = dallas", color: colors[1] },
      { name: "building", detail: "identity.building = b17", color: colors[2] },
      { name: "zone", detail: "logging.level = WARN", color: colors[3] },
      { name: "line", detail: "heartbeat.intervalSecs = 5", color: colors[4] },
      { name: "component", detail: "component.global.pollSecs = 3", color: colors[5] },
    ],
    json: {
      identity: {
        enterprise: "acme-industrial",
        site: "dallas",
        building: "b17",
        zone: "packaging",
        line: "line-7",
      },
      logging: { level: "WARN" },
      heartbeat: { intervalSecs: 5 },
      component: {
        token: "opcua-adapter",
        global: { pollSecs: 3 },
      },
    },
  },
  conflict: {
    title: "Identity conflict",
    narrative:
      "Strict hierarchical mode should reject a lower layer that changes an ancestor identity value.",
    layers: [
      { name: "enterprise", detail: "identity.enterprise = acme-industrial", color: colors[0] },
      { name: "site", detail: "identity.site = dallas", color: colors[1] },
      { name: "zone", detail: "identity.site = houston", color: colors[5] },
      { name: "component", detail: "request cannot be accepted", color: colors[5] },
    ],
    json: {
      error: "IDENTITY_ANCESTOR_CONFLICT",
      path: "$.identity.site",
      owner: "site:dallas",
      attemptedBy: "zone:packaging",
      message: "lower layer attempted to change inherited site identity from dallas to houston",
    },
  },
};

const providerDetails = {
  FILE: {
    title: "FILE",
    heading: "FILE as ConfigComponent catalog backing",
    intro:
      "FILE remains the simplest host and lab backing store for the ConfigComponent catalog. Workload components should not chase parent files; they ask ConfigComponent for a `layers[]` bundle.",
    pills: [
      ["server bootstrap", "-c FILE config-component.json"],
      ["catalog", "/etc/edgecommons/catalog.json"],
      ["watch", "catalog file poll/watch"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "The file source watches the catalog snapshot, not an open-ended set of parent files per workload.",
      "ConfigComponent rejects invalid catalog reloads and keeps serving the previous catalog.",
      "Direct FILE config still loads a single effective config document for bootstrap and tests.",
    ],
  },
  ENV: {
    title: "ENV",
    heading: "ENV as bootstrap or compact catalog source",
    intro:
      "ENV is useful for tiny host deployments, tests, and bootstrap. If used for hierarchy, it should provide a complete catalog snapshot or a pointer to one, not separate per-layer variables.",
    pills: [
      ["server bootstrap", "-c ENV"],
      ["catalog", "EDGECOMMONS_CONFIG_CATALOG={...}"],
      ["pointer", "EDGECOMMONS_CONFIG_CATALOG=@/run/catalog.json"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "No ENV hot reload unless it points to a watchable file-like source.",
      "Avoid resurrecting `EDGECOMMONS_SHARED_CONFIG` or per-layer environment naming conventions.",
      "Direct ENV config remains a single effective config source for small bootstrap cases.",
    ],
  },
  CONFIGMAP: {
    title: "CONFIGMAP",
    heading: "CONFIGMAP as Kubernetes catalog backing",
    intro:
      "Kubernetes maps cleanly to a mounted catalog file. ConfigMaps and projected volumes feed ConfigComponent; workload pods keep the same `-c CONFIG_COMPONENT` source.",
    pills: [
      ["server", "ConfigComponent Deployment"],
      ["catalog", "/etc/edgecommons/catalog.json"],
      ["mount", "ConfigMap/projected volume"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "Avoid `subPath`; it prevents live ConfigMap replacement from reaching the server.",
      "Projected volumes can combine team-owned ConfigMaps into one catalog file path.",
      "The runtime never writes back to Kubernetes resources.",
    ],
  },
  GG_CONFIG: {
    title: "GG_CONFIG",
    heading: "GG_CONFIG as Greengrass bootstrap delivery",
    intro:
      "Greengrass deployment configuration can bootstrap the ConfigComponent and point it at a catalogSource. Consumers still use ConfigComponent over IPC, so each workload does not need direct access to every scope entry.",
    pills: [
      ["server bootstrap", "-c GG_CONFIG"],
      ["catalog pointer", "component.global.configComponent.catalogSource"],
      ["transport", "Greengrass IPC"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "The implemented catalogSource descriptors are file, configmap, and env.",
      "Recipe accessControl must allow config request/reply and push topics over IPC.",
      "The ConfigComponent itself must not bootstrap from CONFIG_COMPONENT.",
    ],
  },
  SHADOW: {
    title: "SHADOW",
    heading: "SHADOW as optional bootstrap delivery",
    intro:
      "SHADOW can deliver the ConfigComponent's own bootstrap config where ShadowManager is already part of the fleet control plane. The catalogSource descriptor itself remains one of the implemented snapshot sources.",
    pills: [
      ["server bootstrap", "-c SHADOW"],
      ["field", "configComponent.catalogSource"],
      ["catalogSource", "file | configmap | env"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "SHADOW is not a separate per-layer lookup path for workload components.",
      "Keep the same catalog shape regardless of how the ConfigComponent bootstrap config is delivered.",
      "A missing catalogSource is a server catalog error, not a component-side fallback.",
    ],
  },
  CONFIG_COMPONENT: {
    title: "CONFIG_COMPONENT",
    heading: "CONFIG_COMPONENT as the workload source",
    intro:
      "CONFIG_COMPONENT is the workload-facing source for hierarchy on all platforms. It returns ordered `layers[]`; language clients merge and validate the effective config.",
    pills: [
      ["GET reply", "{ layers: [...] }"],
      ["push", "set-config with layers[]"],
      ["server", "catalog resolves lineage"],
      ["client", "client merges layers[]"],
    ],
    notes: [
      "The catalog should support scope nodes and component leaves, with cycle detection and version/provenance.",
      "Server may resolve lineage, but it must not merge layers. Clients merge to preserve four-way parity.",
      "Remove the `{ base, component }` response shape and the old two-layer response behavior.",
    ],
  },
};

const deployments = {
  host: {
    title: "Host / supervisord",
    intro:
      "Run the Rust ConfigComponent as a supervised service beside the workload components. Consumers use `-c CONFIG_COMPONENT`; the request/reply and push path rides the local MQTT broker, while the ConfigComponent can load its catalog from files, ENV, or another host-native source.",
    nodes: [
      ["transport", "local MQTT broker"],
      ["server", "ConfigComponent supervised by systemd/supervisord"],
      ["catalog", "/etc/edgecommons/catalog.json or catalog directory"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "FILE remains useful for the ConfigComponent's own bootstrap and catalog backing store.",
      "Consumers no longer need to know the file layout for enterprise, site, line, or component layers.",
      "Supervisord restarts are not needed for valid catalog reloads; ConfigComponent pushes new `layers[]` bundles.",
    ],
  },
  k8s: {
    title: "Kubernetes",
    intro:
      "Deploy ConfigComponent as the namespace or edge-cluster config service. Consumers use `-c CONFIG_COMPONENT`; traffic flows over the in-cluster MQTT service. ConfigMaps and projected volumes become catalog backing stores for the server, not direct component authoring requirements.",
    nodes: [
      ["transport", "in-cluster MQTT service"],
      ["server", "ConfigComponent Deployment"],
      ["catalog", "ConfigMap/projected volume mounted into server"],
      ["consumer", "-c CONFIG_COMPONENT in every workload pod"],
    ],
    notes: [
      "`CONFIGMAP` remains valuable as the ConfigComponent bootstrap/catalog source.",
      "Avoid `subPath` on the server catalog mount; it prevents live ConfigMap updates from reaching the container.",
      "Projected volumes allow separate ConfigMaps per scope while preserving one consumer contract.",
    ],
  },
  gg: {
    title: "Greengrass",
    intro:
      "Deploy ConfigComponent as the on-device config service. Consumers use `-c CONFIG_COMPONENT`; traffic flows over Greengrass IPC. GG_CONFIG or SHADOW can deliver the server bootstrap config, while catalogSource reads the catalog snapshot through file or env on the device.",
    nodes: [
      ["transport", "Greengrass IPC"],
      ["server", "ConfigComponent deployed on the core device"],
      ["catalog", "catalogSource file/env snapshot"],
      ["consumer", "-c CONFIG_COMPONENT"],
    ],
    notes: [
      "ConfigComponent must still bootstrap from non-`CONFIG_COMPONENT` config.",
      "Recipes must grant explicit IPC publish/subscribe access to config command topics.",
      "Completion requires deployed validation on `lab-5950x`, not only local unit tests.",
    ],
  },
};

const implementationDetails = {
  contract: {
    title: "Protocol and data contract",
    lead:
      "The breaking replacement should be one contract: ConfigComponent catalog snapshots use `nodes` and `components`; workload clients receive root-to-leaf `layers[]` bundles.",
    bullets: [
      "Catalog schema: `schemaVersion`, `version`, optional `provenance`, `hierarchy.levels`, `nodes`, and `components`.",
      "Node IDs are catalog-local strings; each node has optional `parent`, required object `scope`, and required object `config`.",
      "Component entries are sanitized short component tokens and have optional `parent` plus required object `config`.",
      "Wire bundles carry `lineageVersion: 1`, `catalogVersion`, `component`, optional `provenance`, and non-empty `layers[]`.",
      "The `config` field is pure partial config. Metadata fields never merge into the effective config.",
    ],
    files: [
      "config-component/src/catalog.rs",
      "config-component/src/coordinator.rs",
      "libs/* config_component source/parser tests",
      "shared JSON conformance vectors",
    ],
    code: `GetConfiguration request:
{ "component": "opcua-adapter" }

Success reply or set-config push:
{
  "lineageVersion": 1,
  "catalogVersion": "2026-07-08T05:00Z",
  "component": "opcua-adapter",
  "layers": [{ "id": "enterprise/acme", "config": {} }]
}`,
  },
  server: {
    title: "ConfigComponent implementation obligations",
    lead:
      "ConfigComponent becomes the only hierarchy resolver. It does not merge effective config; it proves lineage and serves raw partial layers in deterministic order.",
    bullets: [
      "Represent catalog hierarchy with `nodes: Vec/Map<Node>` and keep `components` as sanitized token leaves.",
      "Expose `lineage_for(component)` and `lineages()` helpers that emit `layers[]` payloads.",
      "Validate schemaVersion, explicit version for message updates, catalog object types, node IDs, component tokens, parent existence, cycles, and max depth 64.",
      "Validate scope monotonicity: child scope must preserve every parent scope key and may only add lower-level keys.",
      "Support catalogSource descriptors for file, configmap, and env; keep GG_CONFIG and SHADOW as bootstrap delivery paths, not lineage resolvers.",
      "Preserve reject-and-keep semantics on invalid source reload or invalid message-delivered catalog update.",
    ],
    files: [
      "config-component/src/catalog.rs",
      "config-component/src/coordinator.rs",
      "config-component/src/source.rs",
      "config-component/src/server.rs",
      "config-component/src/tokens.rs",
    ],
    code: `Error body shape:
{
  "ok": false,
  "error": {
    "code": "LINEAGE_CYCLE",
    "message": "component opcua-adapter lineage repeats node line/line-7"
  }
}`,
  },
  clients: {
    title: "Core library implementation obligations",
    lead:
      "All four languages should keep the existing runtime surface: load raw input, merge partial layers, validate once, expose one effective config, and notify reload listeners only after success.",
    bullets: [
      "Remove public support for `{ base, component }` response bodies. Treat them as invalid `LINEAGE_BUNDLE_INVALID` input.",
      "Remove `extends`, `sharedConfig`, and `--no-shared-config` from the public config authoring path and CLI docs.",
      "Parse `layers[]`; require object-valued `config`; preserve metadata for diagnostics but merge only `config`.",
      "Run current deep merge unchanged over an arbitrary list: objects merge recursively, arrays replace, scalars replace, later layers win.",
      "Validate identity ownership before or during merge so lower layers cannot change ancestor identity values.",
      "Keep hot reload reject-and-keep behavior: invalid bundle, invalid effective config, or failed schema validation does not replace the live snapshot.",
    ],
    files: [
      "libs/java/src/main/java/.../config/LayeredConfigCoordinator.java",
      "libs/java/src/main/java/.../config/provider/*ConfigComponent*",
      "libs/python/edgecommons/config/manager/hierarchical_config.py",
      "libs/rust/src/config/layered.rs and effective.rs",
      "libs/ts/src/config/layered.ts and source/config_component.ts",
    ],
    code: `Client merge pseudocode:
bundle = parseLineageBundle(raw)
configs = bundle.layers.map(layer => requireObject(layer.config))
assertIdentityOwnership(configs)
effective = deepMerge(configs)
validateEffectiveConfig(effective)
applySnapshot(effective)`,
  },
  deploy: {
    title: "Deployment implementation obligations",
    lead:
      "The runtime topology should be uniform: ConfigComponent is deployed beside workload components; only transport and catalog backing differ by platform.",
    bullets: [
      "HOST/supervisord: ConfigComponent supervised service, catalog from FILE or ENV, request/reply and push over local MQTT.",
      "Kubernetes: ConfigComponent Deployment, catalog from ConfigMap or projected volume, request/reply and push over in-cluster MQTT.",
      "Greengrass: ConfigComponent component, catalogSource from file/env after non-recursive bootstrap, request/reply and push over IPC.",
      "Workload templates default to `-c CONFIG_COMPONENT` when hierarchical config is enabled.",
      "The ConfigComponent executable rejects recursive bootstrap from CONFIG_COMPONENT and documents the supported bootstrap sources.",
    ],
    files: [
      "templates/*",
      "config-component recipes and deployment docs",
      "core/docs/platform",
      "website docs sync inputs",
    ],
    code: `Common workload shape:
component command ... -c CONFIG_COMPONENT

Platform transport:
HOST        -> local MQTT
Kubernetes  -> in-cluster MQTT
Greengrass  -> IPC`,
  },
  vectors: {
    title: "Shared conformance vectors",
    lead:
      "Vectors must be written before language work so Java, Python, Rust, and TypeScript all implement the same behavior instead of converging by inspection.",
    bullets: [
      "Valid catalogs: component-only and enterprise/site/building/zone/line scopes plus component leaves; device remains runtime identity.",
      "Invalid catalogs: unknown parent, cycle, depth above 64, duplicate ID, malformed config, malformed scope, and bad component token.",
      "Valid bundles: ordered root-to-leaf layers, source/provenance metadata, no server-side merged effective config.",
      "Invalid bundles: empty layers, missing config, non-object config, base/component body, identity conflict, and component token mismatch.",
      "Merge outcomes: object merge, array replace, scalar replace, null handling, and later-layer wins.",
      "Reload outcomes: bad reload keeps previous effective config and records exact error code.",
    ],
    files: [
      "test-vectors/hierarchical-config/catalogs.json",
      "test-vectors/hierarchical-config/lineage-bundles.json",
      "test-vectors/hierarchical-config/merge.json",
      "test-vectors/hierarchical-config/errors.json",
    ],
    code: `Acceptance invariant:
for each language:
  parse same vectors
  produce same effective JSON
  produce same first error code
  keep one effective runtime snapshot`,
  },
};

let currentDepth = hierarchyLevels.length;

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function mergeDeep(target, source) {
  const output = Array.isArray(target) ? [...target] : { ...target };
  for (const [key, value] of Object.entries(source)) {
    if (
      value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      output[key] &&
      typeof output[key] === "object" &&
      !Array.isArray(output[key])
    ) {
      output[key] = mergeDeep(output[key], value);
    } else {
      output[key] = Array.isArray(value) ? [...value] : value && typeof value === "object" ? { ...value } : value;
    }
  }
  return output;
}

function renderLineage() {
  const nodesRoot = document.getElementById("lineageNodes");
  const linksRoot = document.getElementById("lineageLinks");
  const depthLabel = document.getElementById("depthLabel");
  const effectivePreview = document.getElementById("effectivePreview");
  nodesRoot.textContent = "";
  linksRoot.textContent = "";

  const selected = hierarchyLevels.slice(0, currentDepth);
  const layers = [...selected, componentLayer];
  const spacing = 420 / Math.max(layers.length - 1, 1);
  const startY = 54;

  layers.forEach((layer, index) => {
    const x = index % 2 === 0 ? 58 : 132;
    const y = startY + spacing * index;
    if (index > 0) {
      const previousX = (index - 1) % 2 === 0 ? 58 : 132;
      const previousY = startY + spacing * (index - 1);
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("class", "lineage-link");
      path.setAttribute("d", `M${previousX + 184},${previousY + 30} C${previousX + 260},${previousY + 30} ${x - 70},${y + 30} ${x},${y + 30}`);
      linksRoot.appendChild(path);
    }

    const group = document.createElementNS("http://www.w3.org/2000/svg", "g");
    group.setAttribute("class", "node-card");
    const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    rect.setAttribute("x", x);
    rect.setAttribute("y", y);
    rect.setAttribute("width", "184");
    rect.setAttribute("height", "62");
    rect.setAttribute("rx", "8");
    rect.setAttribute("fill", "#ffffff");
    rect.setAttribute("stroke", "#d9dfdc");
    rect.setAttribute("stroke-width", "1");
    group.appendChild(rect);

    const stripe = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    stripe.setAttribute("x", x);
    stripe.setAttribute("y", y);
    stripe.setAttribute("width", "7");
    stripe.setAttribute("height", "62");
    stripe.setAttribute("rx", "4");
    stripe.setAttribute("fill", colors[index % colors.length]);
    group.appendChild(stripe);

    const title = document.createElementNS("http://www.w3.org/2000/svg", "text");
    title.setAttribute("x", x + 18);
    title.setAttribute("y", y + 25);
    title.setAttribute("class", "node-label");
    title.textContent = layer.level;
    group.appendChild(title);

    const value = document.createElementNS("http://www.w3.org/2000/svg", "text");
    value.setAttribute("x", x + 18);
    value.setAttribute("y", y + 46);
    value.setAttribute("class", "node-small");
    value.textContent = layer.value;
    group.appendChild(value);
    nodesRoot.appendChild(group);
  });

  const hierarchy = [...selected.map((item) => item.level), "device"];
  const effective = {
    hierarchy: { levels: hierarchy },
    identity: {},
  };
  for (const layer of selected) {
    effective.identity[layer.level] = layer.value;
  }
  const merged = layers.reduce((acc, layer) => mergeDeep(acc, layer.contributes), effective);
  effectivePreview.textContent = JSON.stringify(merged, null, 2);
  depthLabel.textContent = `${currentDepth} catalog scopes`;
}

function renderScenario(name) {
  const scenario = scenarios[name];
  document.getElementById("scenarioTitle").textContent = scenario.title;
  document.getElementById("scenarioNarrative").textContent = scenario.narrative;
  document.getElementById("scenarioJson").textContent = JSON.stringify(scenario.json, null, 2);

  const stack = document.getElementById("scopeStack");
  stack.textContent = "";
  for (const layer of scenario.layers) {
    const item = document.createElement("div");
    item.className = "scope-layer";
    item.style.borderLeftColor = layer.color;
    item.innerHTML = `<strong>${layer.name}</strong><span>${layer.detail}</span>`;
    stack.appendChild(item);
  }

  document.querySelectorAll("[data-scenario]").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.scenario === name);
  });
}

function renderProvider(name) {
  const provider = providerDetails[name];
  const card = document.getElementById("providerCard");
  card.innerHTML = `
    <div class="provider-card__grid">
      <div>
        <p class="eyebrow">${provider.title}</p>
        <h3>${provider.heading}</h3>
        <p>${provider.intro}</p>
        <ul>${provider.notes.map((note) => `<li>${note}</li>`).join("")}</ul>
      </div>
      <div class="provider-diagram">
        <div class="provider-line">
          ${provider.pills
            .map(
              ([label, value]) => `
                <div class="provider-pill">
                  <strong>${label}</strong>
                  <span>${value}</span>
                </div>
              `,
            )
            .join("")}
        </div>
      </div>
    </div>
  `;

  document.querySelectorAll("[data-provider]").forEach((button) => {
    const active = button.dataset.provider === name;
    button.setAttribute("aria-selected", active ? "true" : "false");
  });
}

function renderDeployment(name) {
  const deployment = deployments[name];
  const view = document.getElementById("deploymentView");
  view.innerHTML = `
    <div class="deployment-grid">
      <div>
        <p class="eyebrow">${deployment.title}</p>
        <h3>${deployment.title}</h3>
        <p>${deployment.intro}</p>
        <ul>${deployment.notes.map((note) => `<li>${note}</li>`).join("")}</ul>
      </div>
      <div class="deployment-map">
        ${deployment.nodes
          .map(
            ([label, value]) => `
            <div class="deployment-node">
              <strong>${label}</strong>
              <span>${value}</span>
            </div>
          `,
          )
          .join("")}
      </div>
    </div>
  `;

  document.querySelectorAll("[data-deployment]").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.deployment === name);
  });
}

function renderImplementation(name) {
  const detail = implementationDetails[name];
  const panel = document.getElementById("implementationPanel");
  panel.innerHTML = `
    <div class="implementation-grid">
      <div>
        <p class="eyebrow">${detail.title}</p>
        <h3>${detail.title}</h3>
        <p>${detail.lead}</p>
        <ul>${detail.bullets.map((item) => `<li>${item}</li>`).join("")}</ul>
      </div>
      <div class="implementation-side">
        <div class="file-list">
          <strong>Primary files and artifacts</strong>
          ${detail.files.map((file) => `<code>${file}</code>`).join("")}
        </div>
        <pre><code>${escapeHtml(detail.code)}</code></pre>
      </div>
    </div>
  `;

  document.querySelectorAll("[data-impl]").forEach((button) => {
    button.classList.toggle("is-active", button.dataset.impl === name);
  });
}

function wireNavHighlight() {
  const links = Array.from(document.querySelectorAll(".rail__nav a"));
  const sections = links
    .map((link) => document.querySelector(link.getAttribute("href")))
    .filter(Boolean);
  const observer = new IntersectionObserver(
    (entries) => {
      const visible = entries
        .filter((entry) => entry.isIntersecting)
        .sort((a, b) => b.intersectionRatio - a.intersectionRatio)[0];
      if (!visible) return;
      links.forEach((link) => {
        link.classList.toggle("is-active", link.getAttribute("href") === `#${visible.target.id}`);
      });
    },
    { rootMargin: "-20% 0px -65% 0px", threshold: [0.1, 0.25, 0.5] },
  );
  sections.forEach((section) => observer.observe(section));
}

function init() {
  document.getElementById("depthMinus").addEventListener("click", () => {
    currentDepth = Math.max(2, currentDepth - 1);
    renderLineage();
  });
  document.getElementById("depthPlus").addEventListener("click", () => {
    currentDepth = Math.min(hierarchyLevels.length, currentDepth + 1);
    renderLineage();
  });
  document.querySelectorAll("[data-scenario]").forEach((button) => {
    button.addEventListener("click", () => renderScenario(button.dataset.scenario));
  });
  document.querySelectorAll("[data-provider]").forEach((button) => {
    button.addEventListener("click", () => renderProvider(button.dataset.provider));
  });
  document.querySelectorAll("[data-deployment]").forEach((button) => {
    button.addEventListener("click", () => renderDeployment(button.dataset.deployment));
  });
  document.querySelectorAll("[data-impl]").forEach((button) => {
    button.addEventListener("click", () => renderImplementation(button.dataset.impl));
  });
  renderLineage();
  renderScenario("line");
  renderProvider("FILE");
  renderDeployment("host");
  renderImplementation("contract");
  wireNavHighlight();
}

init();
