use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=../../assets/CPAWhale.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("gnu") {
        println!("cargo:warning=Windows icon embedding currently requires the GNU target");
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("cpa-whale-icon.o");
    let windres = std::env::var_os("WINDRES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("x86_64-w64-mingw32-windres"));
    let status = Command::new(&windres)
        .current_dir(&manifest_dir)
        .args(["app.rc", "-O", "coff", "-o"])
        .arg(&output)
        .status()
        .expect("launch windres for the CPA Whale icon");
    assert!(status.success(), "windres failed to compile app.rc");
    println!("cargo:rustc-link-arg-bin=cpa-whale={}", output.display());
}
