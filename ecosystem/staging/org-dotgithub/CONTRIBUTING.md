# Contributing to edgecommons

Thanks for contributing! This applies to every repo in the org unless a repo overrides it.

## Building a new component

1. **Scaffold** with the CLI:
   `ggcommons create-component -n com.example.MyAdapter -l <JAVA|PYTHON|RUST|TYPESCRIPT>`.
2. **Name** the repo flat and lowercase by what it does — `opcua-adapter`, `s7-adapter`,
   `rollup-processor`, `kafka-sink`. No `edgecommons-` prefix (the org namespaces it).
3. **Wire CI** by calling the reusable workflow:
   ```yaml
   # .github/workflows/ci.yml
   jobs:
     ci:
       uses: edgecommons/.github/.github/workflows/component-ci.yml@main
       with:
         language: PYTHON   # JAVA | PYTHON | RUST | TYPESCRIPT
   ```
4. **Topic** the repo: `edgecommons`, the category topic (`edgecommons-adapter` /
   `edgecommons-processor` / `edgecommons-sink`), `aws-iot-greengrass`, `iiot`, and a protocol topic.
5. **Register** it: open a PR adding an entry to
   [`edgecommons/registry`](https://github.com/edgecommons/registry).

## Standards

- Components build on `ggcommons` and follow its conventions (builders, the standard CLI contract,
  the message envelope). Adapters follow the southbound contract (`docs/SOUTHBOUND.md`).
- Tests required; keep CI green before requesting review.
- Match the surrounding code style of the language/library.

## Pull requests

Use the PR template, keep changes focused, and describe what you verified. By contributing you agree
your work is licensed under the repository's license and that you follow the
[Code of Conduct](CODE_OF_CONDUCT.md).
