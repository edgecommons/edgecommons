# Requires the edgecommons Java artifact resolvable from GitHub Packages Maven (com.mbreissi.edgecommons:edgecommons);
# see docs/platform/DESIGN-packaging.md §13. --enable-native-access is for the streaming FFM binding.
#
# Multi-stage build: stage 1 compiles the shaded, self-contained component jar with Maven against the
# PUBLISHED edgecommons artifact; stage 2 is a slim JRE that runs it as a non-root user. With
# --platform auto the library detects KUBERNETES, defaults config to CONFIGMAP and transport to MQTT,
# and resolves identity from the Downward API — so the container needs NO default args.

# ---- Stage 1: build the shaded component jar (target/<<JARNAME>>-1.0.0.jar) ----
FROM maven:3.9-eclipse-temurin-25 AS build
WORKDIR /build
COPY pom.xml ./
COPY src ./src
RUN mvn -q -DskipTests package

# ---- Stage 2: minimal JRE runtime, non-root (UID 65532) ----
FROM eclipse-temurin:25-jre
WORKDIR /app
COPY --from=build /build/target/<<JARNAME>>-1.0.0.jar /app/app.jar
USER 65532:65532
ENTRYPOINT ["java", "--enable-native-access=ALL-UNNAMED", "-jar", "/app/app.jar"]
