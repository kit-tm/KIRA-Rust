use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure()
        .file_descriptor_set_path(PathBuf::from(env::var("OUT_DIR")?).join("kellyconnector.bin"))
        .compile(&["proto/kellyconnector.proto"], &["proto"])?;
    Ok(())
}