//! Minimal example component. Phase 0: initializes GGCommons from the standard
//! CLI args, loads + validates config, and prints a summary.
//!
//! Run:
//! ```bash
//! cargo run --example skeleton -- -m STANDALONE ./messaging.json -c FILE ./config.json -t my-thing
//! ```

use ggcommons::prelude::*;

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

    Ok(())
}
