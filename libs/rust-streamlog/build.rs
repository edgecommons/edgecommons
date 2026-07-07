use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_root = PathBuf::from("../../proto");
    let protos = [
        "edgecommons/v1/value.proto",
        "edgecommons/v1/command.proto",
        "edgecommons/v1/config.proto",
        "edgecommons/v1/event.proto",
        "edgecommons/v1/metrics.proto",
        "edgecommons/v1/state.proto",
        "edgecommons/v1/telemetry.proto",
        "edgecommons/v1/message.proto",
    ]
    .map(|p| proto_root.join(p));

    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    prost_build::Config::new()
        .btree_map(["."])
        .compile_protos(&protos, &[proto_root])?;

    println!("cargo:rerun-if-changed=../../proto/edgecommons/v1");
    Ok(())
}
