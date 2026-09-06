# Website documentation guidance

Read [README.md](README.md) and the parent [AGENTS.md](../AGENTS.md) before editing. Use current files
and Git history directly. Do not use CodeGraph or Graphify, including lingering generated indexes.

Edit component/tool docs in owning repositories or `cli/docs/`, then run the existing sync/build
pipeline. Generated `src/content/docs/components/` and `tools/` pages are not sources. Shared guides
and references describe the implemented four-language contract. Full-envelope JSON examples need a
local **JSON projection** label; native configuration JSON and body-only examples retain their actual
meaning. Normal MQTT and Greengrass IPC messaging carries protobuf bytes.

Verify a local-source build with `REGISTRY_JSON` and a complete `COMPONENT_DOCS_MAP`; inspect catalog
completeness as well as link/anchor validation. Do not claim runtime validation from a docs build.
Use `npm run dev` for preview and stop the process when finished. Brand token changes originate in
the sibling brand repository's JSON source and are generated/synced here.
