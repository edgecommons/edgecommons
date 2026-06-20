# Java on-device validation

`shadow-verify/` (a small shaded Maven app depending on `com.aws.proserve:ggcommons`)
+ `com.ggcommons.JavaShadowVerify-1.0.0.yaml` validate the **sanitized default SHADOW
name** end-to-end on a live Greengrass nucleus: run as `java -jar shadow-verify.jar
-c SHADOW` (no name), the SHADOW provider defaults the shadow name to the component
name and sanitizes it (`com.ggcommons.JavaShadowVerify` →
`com_ggcommons_JavaShadowVerify`), then loads config from that named shadow.

## Build & deploy

```bash
# Build the lib into ~/.m2, then the shaded harness jar:
mvn -DskipTests -f libs/java/pom.xml install
mvn -f libs/java/deploy/shadow-verify/pom.xml package      # -> target/shadow-verify.jar
# Deploy as a raw-jar artifact at <artifactDir>/com.ggcommons.JavaShadowVerify/1.0.0/shadow-verify.jar
sudo greengrass-cli deployment create --recipeDir recipes --artifactDir artifacts \
  --merge "com.ggcommons.JavaShadowVerify=1.0.0"
# Result -> /tmp/java_shadow_verify_result.json
```

Result observed (name fix confirmed via the log
`Will load configuration from Named shadow (shadow name: 'com_ggcommons_JavaShadowVerify')`,
and the loaded marker values):
`{"lang":"java","connected":true,"config_loaded":{"publish_interval":37,"site":"java-shadow",...}}`

## ⚠️ Cross-language shadow-format inconsistency (separate, pre-existing)

Surfaced by this validation: Java's `ShadowConfigProvider.getConfiguration()` reads
`state.desired.ComponentConfig` with `.toString()` and parses it as a JSON **object**.
Python/Rust/TS instead store/read `ComponentConfig` as a **stringified JSON string**
(`extractConfigStr` → `JSON.parse`). So a shadow written by Java is not readable by the
other three (and vice versa) — they are NOT interoperable on the shadow payload format.
This is independent of the shadow-NAME fix and needs a decision on the canonical format
before fixing (likely Java should use `.getAsString()` to match the stringified
contract the other three + the source docs describe).
