//! Compiles Protocol Buffers and FlatBuffers schema definitions into
//! native Rust types.
//!
//! Source files are found in

use std::{
    path::{Path, PathBuf},
    process::Command,
};

type Error = Box<dyn std::error::Error>;
type Result<T, E = Error> = std::result::Result<T, E>;

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    generate_grpc_types(&root)?;
    generate_wal_types(&root)?;

    Ok(())
}

/// Schema used with gRPC requests
///
/// Creates `influxdata.platform.storage.rs`
fn generate_grpc_types(root: &Path) -> Result<()> {
    let proto_file = root.join("influxdb_iox.proto");

    println!("cargo:rerun-if-changed={}", proto_file.display());
    tonic_build::compile_protos(proto_file)?;

    Ok(())
}

/// Schema used in the WAL
///
/// Creates `wal_generated.rs`
fn generate_wal_types(root: &Path) -> Result<()> {
    let wal_file = root.join("wal.fbs");

    println!("cargo:rerun-if-changed={}", wal_file.display());
    let out_dir: PathBuf = std::env::var_os("OUT_DIR")
        .expect("Could not determine `OUT_DIR`")
        .into();

    let status = Command::new("flatc")
        .arg("--rust")
        .arg("-o")
        .arg(&out_dir)
        .arg(wal_file)
        .status();

    match status {
        Ok(status) if !status.success() => panic!("`flatc` failed to compile the .fbs to Rust"),
        Ok(_status) => {} // Successfully compiled
        Err(err) => panic!("Could not execute `flatc`: {}", err),
    }

    Ok(())
}
