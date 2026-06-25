//! Minimal example component.
//!
//! Initializes GGCommons from the standard CLI args, then (on the MQTT transport)
//! publishes one demo message through the messaging service.
//!
//! Run against the local broker:
//! ```bash
//! cargo run --example skeleton -- \
//!   --platform HOST --transport MQTT ./test-configs/messaging.json \
//!   -c FILE ./test-configs/config.json \
//!   -t my-thing
//! ```

use ggcommons::messaging::message::MessageBuilder;
use ggcommons::prelude::*;
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let gg = GgCommonsBuilder::new("com.example.SkeletonComponent")
        .args(std::env::args_os())
        .build()
        .await?;

    let cfg = gg.config();
    println!("component:   {}", gg.component_name());
    println!("thing:       {}", cfg.thing_name);
    println!("platform:    {:?}", gg.args().platform);
    println!("transport:   {:?}", gg.args().transport);
    println!("instances:   {:?}", cfg.instance_ids());

    // On the MQTT transport the messaging service is available; publish a demo message.
    if let Ok(messaging) = gg.messaging() {
        let topic = format!("demo/{}/hello", cfg.thing_name);
        let msg = MessageBuilder::new("Hello", "1.0")
            .payload(json!({ "greeting": "hello from rust ggcommons" }))
            .from_config(&cfg)
            .build();
        messaging.publish(&topic, &msg).await?;
        println!("published:   {topic}");
    }

    Ok(())
}
