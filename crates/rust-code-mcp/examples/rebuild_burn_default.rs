//! One-shot: force-rebuild the burn snapshot to the *default* data dir so the
//! debug_burn_target tool (which reads default-dir) sees the latest output.
use std::path::Path;
use rmc_graph::graph::{BuildOptions, build_and_persist};

fn main() {
    let r = build_and_persist(
        Path::new("/home/molaco/Documents/burn"),
        BuildOptions {
            force_rebuild: true,
            data_dir_override: None,
            env: Default::default(),
        },
    )
    .expect("build");
    eprintln!(
        "rebuilt: nodes={} bindings={} reused={}",
        r.node_count, r.binding_count, r.reused
    );
}
