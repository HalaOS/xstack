fn main() {
    // Use this in build.rs
    protobuf_codegen::Codegen::new()
        // Use `protoc` parser, optional.
        .protoc()
        // Use `protoc-bin-vendored` bundled protoc command, optional.
        .protoc_path(&protoc_bin_vendored::protoc_bin_path().unwrap())
        // All inputs and imports from the inputs must reside in `includes` directories.
        // Inputs must reside in some of include paths.
        .includes(&["proto"])
        .input("proto/rpc.proto")
        // Specify output directory relative to Cargo output directory.
        .out_dir("src/proto")
        .run_from_script();
}
