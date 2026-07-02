use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=src/wsg_shim.c");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = manifest_dir.join("src").join("wsg_shim.c");
    let output = out_dir.join("libdicta_qianwen_wsg_shim.dylib");
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => "",
    };

    let mut command = Command::new(compiler);
    command.arg("-dynamiclib").arg("-fPIC").arg("-O2");
    if !arch.is_empty() {
        command.arg("-arch").arg(arch);
    }
    command
        .arg("-Wl,-install_name,@rpath/libdicta_qianwen_wsg_shim.dylib")
        .arg("-o")
        .arg(&output)
        .arg(&source);

    let status = command
        .status()
        .expect("failed to invoke C compiler for Qianwen WSG shim");
    assert!(status.success(), "failed to build Qianwen WSG shim");

    println!(
        "cargo:rustc-env=DICTA_QIANWEN_WSG_SHIM_DYLIB={}",
        output.display()
    );
}
