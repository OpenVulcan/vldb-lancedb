fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: the build script sets PROTOC once during process startup, before any
    // threads are spawned or foreign code observes the environment.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_protos(&["proto/v1/lancedb.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/v1/lancedb.proto");
    Ok(())
}
