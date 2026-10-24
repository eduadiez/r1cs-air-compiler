//! Generates the Rust types for the vendored PILout protobuf schema.
fn main() {
    let root = "vendor/pilout";
    println!("cargo:rerun-if-changed={root}/pilout.proto");
    prost_build::compile_protos(&[format!("{root}/pilout.proto")], &[root])
        .expect("generating Rust code from vendor/pilout/pilout.proto (requires protoc)");
}
