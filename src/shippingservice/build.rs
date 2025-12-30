use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let proto_path = if Path::new("../../protos/demo.proto").exists() {
        "../../protos/demo.proto"  // Local development
    } else {
        "protos/demo.proto"  // Docker build
    };
    
    tonic_prost_build::compile_protos(proto_path)?;
    Ok(())
}