use std::path::Path;
fn main() {
    let (proto_file, proto_dir) = if Path::new("../../protos/demo.proto").exists() {
        ("../../protos/demo.proto", "../../protos")
    } else {
        ("protos/demo.proto", "protos")
    };
    
    tonic_build::configure()
        .build_transport(false)
        .compile_protos(&[proto_file], &[proto_dir])
        .unwrap();
}