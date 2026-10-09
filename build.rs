fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Keep setup portable: neither Windows nor Linux needs a system protoc.
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    let mut config = tonic_build::Config::new();
    config.protoc_executable(protoc);
    tonic_build::configure()
        .type_attribute(".", "#[derive(serde::Serialize, serde::Deserialize)]")
        .type_attribute(".", "#[serde(default)]")
        .compile_protos_with_config(config, &["proto/farm.proto"], &["proto"])?;
    println!("cargo:rerun-if-changed=proto/farm.proto");
    Ok(())
}
