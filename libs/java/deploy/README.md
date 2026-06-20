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

## Cross-language shadow-format bug (found here, now FIXED)

This validation surfaced a separate pre-existing bug: Java's
`ShadowConfigProvider` extracted `state.desired.ComponentConfig` with `.toString()`,
which on a Gson string primitive keeps the quotes/escapes, so `Utils.destringify`
failed ("Unable to deserialize string into json object") and init failed with "No
configuration found". `ComponentConfig` is a **stringified JSON string** in all four
libs (the canonical format — it avoids the IoT shadow JSON-depth limit). Fixed by
using `.getAsString()` (the raw inner JSON) at both extraction sites (load + delta).

Verified end-to-end: with a **stringified** cloud shadow (the format Python/Rust/TS
write), the fixed Java component loaded `{"connected":true,"config_loaded":
{"publish_interval":41,"site":"java-stringified",...}}`. All four libraries now agree
on both the shadow name and the payload format.
