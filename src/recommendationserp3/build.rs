fn main() {
    tonic_prost_build::configure()
        .build_transport(false)
        .build_client(false)
        .compile_protos(&["../../protos/demo.proto"], &["../../protos"])
        .unwrap();
}
