//! Cross-language interop node (Rust) for ggcommons. See python_node.py for the
//! shared CLI contract:
//!   interop-rust-node responder <request_topic>
//!   interop-rust-node request   <request_topic> <token>
//! STANDALONE local-only against localhost:1883.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use ggcommons::messaging::config::MessagingConfig;
use ggcommons::messaging::message::MessageBuilder;
use ggcommons::messaging::message_handler;
use ggcommons::messaging::provider::mqtt::MqttProvider;
use ggcommons::messaging::service::{DefaultMessagingService, MessagingService};

const LANG: &str = "rust";

async fn provider(suffix: &str) -> Arc<DefaultMessagingService> {
    let host = std::env::var("GGCOMMONS_IT_MQTT_HOST").unwrap_or_else(|_| "localhost".to_string());
    let port = std::env::var("GGCOMMONS_IT_MQTT_PORT").unwrap_or_else(|_| "1883".to_string());
    let pid = std::process::id();
    let cfg = format!(
        r#"{{ "messaging": {{ "local": {{ "host": "{host}", "port": {port}, "clientId": "interop-{LANG}-{suffix}-{pid}" }} }} }}"#
    );
    let mc: MessagingConfig = serde_json::from_str(&cfg).expect("valid config");
    let provider = MqttProvider::connect(&mc).await.expect("connect to local broker");
    Arc::new(DefaultMessagingService::new(Arc::new(provider)))
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
                            .thing_name("interop-rust")
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
            let req = MessageBuilder::new("InteropRequest", "1.0")
                .payload(json!({ "token": token, "from": LANG }))
                .thing_name("interop-rust")
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
        other => {
            eprintln!("unknown role: {other}");
            std::process::exit(2);
        }
    }
}
