use std::{path::Path, process::Command};

fn revision(path: &Path) -> String {
    let Ok(output) = Command::new("git").args(["rev-parse", "HEAD"]).current_dir(path).output() else {
        return "unknown (git unavailable)".to_owned();
    };
    if !output.status.success() { return "unknown (no git revision)".to_owned(); }
    for name in ["HEAD", "refs", "packed-refs", "index"] {
        let output = Command::new("git").args(["rev-parse", "--git-path", name]).current_dir(path).output().unwrap();
        let git_path = String::from_utf8(output.stdout).unwrap();
        println!("cargo:rerun-if-changed={}", path.join(git_path.trim()).display());
    }
    let sha = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let clean = Command::new("git").args(["diff", "--quiet", "HEAD"]).current_dir(path).status().unwrap();
    format!("{sha}{}", if clean.success() { "" } else { "+dirty" })
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();
    let ra = root.join("../rust-analyzer-bur");
    let salsa = root.join("../salsa-bur");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", root.join("Cargo.lock").display());
    println!("cargo:rerun-if-changed={}", ra.join("crates").display());
    println!("cargo:rerun-if-changed={}", salsa.join("src").display());
    println!("cargo:rustc-env=RMC_GRAPH_REVISION={}", revision(root));
    println!("cargo:rustc-env=RMC_RA_REVISION={}", revision(&ra));
    println!("cargo:rustc-env=RMC_SALSA_REVISION={}", revision(&salsa));
}
