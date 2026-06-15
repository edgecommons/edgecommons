//! Minimal example component.
//!
//! Initializes GGCommons from the standard CLI args, then (in STANDALONE mode)
//! publishes one demo message through the messaging service.
//!
//! Run against the local broker:
//! ```bash
//! cargo run --example skeleton -- \
//!   -m STANDALONE ./test-configs/messaging.json \
//!   -c FILE ./test-configs/config.json \
//!   -t my-thing
//! ```

use ggcommons::messaging::message::MessageBuilder;
use ggcommons::messaging::Destination;
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
    println!("mode:        {:?}", gg.args().mode);
    println!("instances:   {:?}", cfg.instance_ids());

    // In STANDALONE mode the messaging service is available; publish a demo message.
    if let Ok(messaging) = gg.messaging() {
        let topic = format!("demo/{}/hello", cfg.thing_name);
        let msg = MessageBuilder::new("Hello", "1.0")
            .payload(json!({ "greeting": "hello from rust ggcommons" }))
            .from_config(&cfg)
            .build();
        messaging.publish(&topic, &msg, Destination::Local).await?;
        println!("published:   {topic}");
    }

    Ok(())
}
