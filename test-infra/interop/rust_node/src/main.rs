//! Cross-language interop node (Rust) for edgecommons. See python_node.py for the
//! shared CLI contract:
//!   interop-rust-node responder <request_topic>
//!   interop-rust-node request   <request_topic> <token>
//!   interop-rust-node uns-pub   <identityJson> <class> [channel]
//!   interop-rust-node uns-sub   <topic>
//!   interop-rust-node uns-guard
//! Local-only MQTT transport against localhost:1883. Messages are built without a
//! config — the envelope legally omits `identity` unless one is stamped explicitly
//! (the UNS roles); `tags.thing` no longer exists (UNS hard cut).

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use edgecommons::error::EdgeCommonsError;
use edgecommons::messaging::config::MessagingConfig;
use edgecommons::messaging::message::{MessageBuilder, MessageIdentity};
use edgecommons::messaging::message_handler;
use edgecommons::messaging::provider::mqtt::MqttProvider;
use edgecommons::messaging::service::{DefaultMessagingService, MessagingService};
use edgecommons::uns::{Uns, UnsClass};
#[cfg(feature = "greengrass")]
use edgecommons::messaging::provider::ipc::IpcProvider;

const LANG: &str = "rust";

fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("hex length must be even".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn provider(suffix: &str) -> Arc<DefaultMessagingService> {
    let host = std::env::var("EDGECOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("EDGECOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let pid = std::process::id();
    let cfg = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "interop-{LANG}-{suffix}-{pid}" }} }} }}"#
    );
    let mc: MessagingConfig = serde_json::from_str(&cfg).expect("valid config");
    let provider = MqttProvider::connect(&mc).await.expect("connect to local broker");
    Arc::new(DefaultMessagingService::new(Arc::new(provider)))
}

fn gg_topic(run_id: &str, publisher: &str, subscriber: &str) -> String {
    format!("edgecommons/interop/binary/{run_id}/{publisher}/{subscriber}")
}

fn publisher_from_gg_topic(topic: &str) -> Option<String> {
    topic.split('/').rev().nth(1).map(ToString::to_string)
}

fn gg_ready_path(run_id: &str, lang: &str) -> String {
    format!("/tmp/edgecommons_gg_ipc_binary_ready_{lang}_{run_id}")
}

#[cfg(feature = "greengrass")]
async fn ipc_provider() -> Arc<DefaultMessagingService> {
    let provider = IpcProvider::connect().await.expect("connect to Greengrass IPC");
    Arc::new(DefaultMessagingService::new(Arc::new(provider)))
}

#[cfg(feature = "greengrass")]
async fn wait_for_gg_ready(run_id: &str, expected_langs: &[String]) -> Vec<String> {
    let ready_wait_secs: f64 = std::env::var("EDGECOMMONS_GG_READY_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180.0);
    let deadline = std::time::Instant::now() + Duration::from_secs_f64(ready_wait_secs);
    while std::time::Instant::now() < deadline {
        let missing: Vec<String> = expected_langs
            .iter()
            .filter(|lang| !std::path::Path::new(&gg_ready_path(run_id, lang)).exists())
            .cloned()
            .collect();
        if missing.is_empty() {
            return missing;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    expected_langs
        .iter()
        .filter(|lang| !std::path::Path::new(&gg_ready_path(run_id, lang)).exists())
        .cloned()
        .collect()
}

#[cfg(feature = "greengrass")]
async fn run_gg_binary_matrix(args: &[String]) -> ! {
    use std::collections::{BTreeMap, BTreeSet};

    let run_id = args[2].clone();
    let expected_langs: Vec<String> = args[3]
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let expected: BTreeSet<String> = expected_langs.iter().cloned().collect();
    let ready_langs: Vec<String> = std::env::var("EDGECOMMONS_GG_READY_LANGS")
        .unwrap_or_else(|_| args[3].clone())
        .split(',')
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect();
    let ready_lang = std::env::var("EDGECOMMONS_GG_READY_LANG").unwrap_or_else(|_| LANG.to_string());
    let expected_hex = args[4].to_lowercase();
    let expected_bytes = decode_hex(&expected_hex).expect("expected hex");
    let subscribe_delay_secs: f64 = std::env::var("EDGECOMMONS_GG_SUBSCRIBE_DELAY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8.0);
    let wait_secs: f64 = std::env::var("EDGECOMMONS_GG_WAIT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(35.0);

    let svc = ipc_provider().await;
    let received = Arc::new(std::sync::Mutex::new(BTreeMap::<String, serde_json::Value>::new()));
    let errors = Arc::new(std::sync::Mutex::new(BTreeMap::<String, String>::new()));
    let rh = received.clone();
    let eh = errors.clone();
    let expected_for_handler = expected_bytes.clone();
    svc.subscribe(
        &gg_topic(&run_id, "+", LANG),
        message_handler(move |topic, m| {
            let rh = rh.clone();
            let eh = eh.clone();
            let expected_for_handler = expected_for_handler.clone();
            async move {
                let publisher = publisher_from_gg_topic(&topic).unwrap_or_else(|| "unknown".to_string());
                match (m.is_binary_body(), m.binary_body()) {
                    (is_binary, Ok(Some(bytes))) => {
                        let ok = is_binary && bytes == expected_for_handler;
                        rh.lock().unwrap().entry(publisher).or_insert_with(|| {
                            json!({"is_binary": is_binary, "hex": encode_hex(&bytes), "ok": ok})
                        });
                    }
                    (is_binary, Ok(None)) => {
                        rh.lock().unwrap().entry(publisher).or_insert_with(|| {
                            json!({"is_binary": is_binary, "hex": null, "ok": false})
                        });
                    }
                    (is_binary, Err(e)) => {
                        eh.lock().unwrap().insert(publisher.clone(), e.to_string());
                        rh.lock().unwrap().entry(publisher).or_insert_with(|| {
                            json!({"is_binary": is_binary, "hex": null, "ok": false})
                        });
                    }
                }
            }
        }),
        64,
        1,
    )
    .await
    .expect("subscribe");
    println!("READY");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::fs::write(gg_ready_path(&run_id, &ready_lang), format!("{:?}", std::time::SystemTime::now()))
        .expect("write ready");
    let ready_missing = wait_for_gg_ready(&run_id, &ready_langs).await;
    tokio::time::sleep(Duration::from_secs_f64(subscribe_delay_secs)).await;

    if ready_missing.is_empty() {
        let msg = MessageBuilder::new("InteropBinary", "1.0")
            .binary_payload(&expected_bytes)
            .expect("binary payload")
            .tag("from", json!(LANG))
            .build();
        for target in &expected_langs {
            svc.publish(&gg_topic(&run_id, LANG, target), &msg)
                .await
                .expect("publish");
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs_f64(wait_secs);
    while std::time::Instant::now() < deadline {
        let got: BTreeSet<String> = received.lock().unwrap().keys().cloned().collect();
        if expected.is_subset(&got) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let received_snapshot = received.lock().unwrap().clone();
    let errors_snapshot = errors.lock().unwrap().clone();
    let missing: Vec<String> = expected_langs
        .iter()
        .filter(|lang| !received_snapshot.contains_key(*lang))
        .cloned()
        .collect();
    let ok = ready_missing.is_empty()
        && missing.is_empty()
        && errors_snapshot.is_empty()
        && expected_langs.iter().all(|lang| {
            received_snapshot
                .get(lang)
                .and_then(|v| v.get("ok"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        });
    let result = json!({
        "ok": ok,
        "lang": LANG,
        "run_id": run_id,
        "expected_hex": expected_hex,
        "ready_missing": ready_missing,
        "received": received_snapshot,
        "missing": missing,
        "errors": errors_snapshot,
    });
    let path = format!("/tmp/edgecommons_gg_ipc_binary_{LANG}_{}.json", args[2]);
    std::fs::write(&path, serde_json::to_string(&result).unwrap()).expect("write result");
    println!("{result}");
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(not(feature = "greengrass"))]
async fn run_gg_binary_matrix(_args: &[String]) -> ! {
    eprintln!("gg-binary-matrix requires the greengrass cargo feature");
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let role = args.get(1).map(String::as_str).unwrap_or("");

    match role {
        "responder" => {
            let topic = args[2].clone();
            let svc = provider("resp").await;
            let responder = svc.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, request| {
                    let responder = responder.clone();
                    async move {
                        let reply = MessageBuilder::new("InteropReply", "1.0")
                            .payload(json!({ "echo": request.body, "responder": LANG }))
                            .build();
                        if let Err(e) = responder.reply(&request, reply).await {
                            eprintln!("reply failed: {e}");
                        }
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        "request" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("req").await;
            // Canonical cross-language payload permutations (echoed back by the responder;
            // test_interop asserts a deep round-trip). null is tested inside an array.
            let types = json!({
                "b": true, "bf": false,
                "i": 42, "ni": -7, "fl": 3.5,
                "slash": "a/b", "quote": "x\"y",
                "arr": [1, "two", false, null],
                "nullv": null,
                "nested": { "k": [1, { "d": 2 }] },
                "ea": [], "eo": {}
            });
            let req = MessageBuilder::new("InteropRequest", "1.0")
                .payload(json!({ "token": token, "from": LANG, "types": types }))
                .build();
            let corr = req.header.correlation_id.clone();
            let fut = svc.request(&topic, req).await.expect("request issued");
            match tokio::time::timeout(Duration::from_secs(8), fut).await {
                Ok(Ok(reply)) => {
                    let matched = reply.header.correlation_id == corr;
                    println!(
                        "{}",
                        json!({"ok": true, "correlation_match": matched, "reply_body": reply.body})
                    );
                    let echo_token = reply
                        .body
                        .get("echo")
                        .and_then(|e| e.get("token"))
                        .and_then(|t| t.as_str());
                    let ok = matched
                        && reply.body.get("responder").is_some()
                        && echo_token == Some(token.as_str());
                    std::process::exit(if ok { 0 } else { 1 });
                }
                _ => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "raw-sub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("rawsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        *rh.lock().unwrap() = Some((m.is_raw(), m.get_raw().cloned()));
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().clone();
            match result {
                Some((is_raw, raw)) => {
                    let raw_token = raw
                        .as_ref()
                        .and_then(|v| v.get("token"))
                        .and_then(|t| t.as_str());
                    let ok = is_raw && raw_token == Some(token.as_str());
                    println!("{}", json!({"ok": ok, "is_raw": is_raw, "raw_token": raw_token}));
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "raw-pub" => {
            let topic = args[2].clone();
            let token = args[3].clone();
            let svc = provider("rawpub").await;
            svc.publish_raw(&topic, &json!({ "token": token, "from": LANG }))
                .await
                .expect("publish_raw");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        "binary-sub" => {
            let topic = args[2].clone();
            let expected_hex = args[3].to_lowercase();
            let svc = provider("binsub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        let result = match (m.is_binary_body(), m.binary_body()) {
                            (is_binary, Ok(Some(bytes))) => {
                                json!({"is_binary": is_binary, "hex": encode_hex(&bytes)})
                            }
                            (is_binary, Ok(None)) => json!({"is_binary": is_binary, "hex": null}),
                            (is_binary, Err(e)) => {
                                json!({"is_binary": is_binary, "hex": null, "error": e.to_string()})
                            }
                        };
                        *rh.lock().unwrap() = Some(result);
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(mut payload) => {
                    let ok = payload
                        .get("is_binary")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                        && payload.get("hex").and_then(|v| v.as_str()) == Some(expected_hex.as_str());
                    payload["ok"] = json!(ok);
                    println!("{}", payload);
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        "binary-pub" => {
            let topic = args[2].clone();
            let bytes = decode_hex(&args[3]).expect("body hex");
            let svc = provider("binpub").await;
            let msg = MessageBuilder::new("InteropBinary", "1.0")
                .binary_payload(&bytes)
                .expect("binary payload")
                .tag("from", json!(LANG))
                .build();
            svc.publish(&topic, &msg).await.expect("publish");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        "gg-binary-matrix" => run_gg_binary_matrix(&args).await,
        // uns-pub <identityJson> <class> [channel] — mint the topic with the real Uns
        // builder (includeRoot=false), stamp the identity via the real MessageBuilder,
        // publish, and print {"ok":true,"topic":...,"envelope":...}.
        "uns-pub" => {
            let identity_value: serde_json::Value =
                serde_json::from_str(&args[2]).expect("identity argument must be JSON");
            let Some(identity) = MessageIdentity::from_wire(&identity_value) else {
                eprintln!("bad identity: {}", args[2]);
                std::process::exit(2);
            };
            let Some(cls) = UnsClass::from_token(&args[3]) else {
                eprintln!("bad class: {}", args[3]);
                std::process::exit(2);
            };
            let channel = args.get(4).cloned();
            let uns = Uns::new(identity.clone(), false);
            let topic = match channel.as_deref() {
                Some(c) if !c.is_empty() => uns.topic_with_channel(cls, c),
                _ => uns.topic(cls),
            }
            .expect("mint UNS topic");
            let svc = provider("unspub").await;
            let msg = MessageBuilder::new("UnsInterop", "1.0")
                .payload(json!({ "from": LANG }))
                .identity(identity)
                .build();
            svc.publish(&topic, &msg).await.expect("publish");
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("{}", json!({ "ok": true, "topic": topic, "envelope": msg }));
        }
        // uns-sub <topic> — receive one envelope and print its parsed identity.
        "uns-sub" => {
            let topic = args[2].clone();
            let svc = provider("unssub").await;
            let recv = Arc::new(std::sync::Mutex::new(None));
            let rh = recv.clone();
            svc.subscribe(
                &topic,
                message_handler(move |_t, m| {
                    let rh = rh.clone();
                    async move {
                        *rh.lock().unwrap() = Some(m);
                    }
                }),
                16,
                1,
            )
            .await
            .expect("subscribe");
            println!("READY");
            use std::io::Write;
            let _ = std::io::stdout().flush();
            for _ in 0..100 {
                if recv.lock().unwrap().is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let result = recv.lock().unwrap().take();
            match result {
                Some(m) => {
                    let ok = m.identity.is_some();
                    println!(
                        "{}",
                        json!({ "ok": ok, "identity": m.identity, "body": m.body })
                    );
                    std::process::exit(if ok { 0 } else { 1 });
                }
                None => {
                    println!("{}", json!({"ok": false, "error": "timeout"}));
                    std::process::exit(1);
                }
            }
        }
        // uns-guard — attempt a raw publish to a reserved-class topic through the
        // guarded public service; must fail with EdgeCommonsError::ReservedTopic (§4.1).
        "uns-guard" => {
            let svc = provider("guard").await;
            let topic = "ecv1/dev1/comp1/main/state";
            match svc.publish_raw(topic, &json!({ "from": LANG })).await {
                Err(EdgeCommonsError::ReservedTopic(detail)) => {
                    println!(
                        "{}",
                        json!({ "error": "ReservedTopic", "detail": detail, "topic": topic })
                    );
                    std::process::exit(3);
                }
                Err(e) => {
                    println!("{}", json!({ "error": format!("{e}") }));
                    std::process::exit(4);
                }
                Ok(()) => {
                    println!("{}", json!({ "ok": true }));
                }
            }
        }
        other => {
            eprintln!("unknown role: {other}");
            std::process::exit(2);
        }
    }
}
