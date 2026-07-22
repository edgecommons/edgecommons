# <<COMPONENTNAME>> — Claude Code

@AGENTS.md

## Local development

- Build against the sibling core library: from a monorepo checkout, `cd ../core/libs/java && mvn
  install -DskipTests`, then build this component with `mvn -Dedgecommons.version=<the version that
  install printed> package`. See the `edgecommons.version` property comment in `pom.xml`.
- This template's dependency is resolved from GitHub Packages by version (Maven has no path-dependency
  mechanism and no git-rev dependency form); there is no `.cargo`-style local-override file to look
  for here — the `mvn install` step above is the whole local-dev story for this language.
