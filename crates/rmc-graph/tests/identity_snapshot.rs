//! Identities and counts checked against source contracts, not extractor parity.
use std::{collections::{BTreeMap, HashSet}, fs};
use rmc_graph::graph::{self, NodeKind};

fn write_fixture(root: &std::path::Path) {
    fs::write(root.join("Cargo.toml"), "[package]\nname='identity'\nversion='0.1.0'\nedition='2021'\n[workspace]\n").unwrap();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub mod shared; pub struct T; pub enum E { A, B } pub trait Act { fn act(&self); } impl Act for T { fn act(&self) {} }\n").unwrap();
    fs::write(root.join("src/main.rs"), "mod shared; fn main() { shared::run(); identity::shared::run(); }\n").unwrap();
    fs::write(root.join("src/shared.rs"), r#"
pub fn leaf() {}
pub fn run() { leaf(); fn nested() { leaf(); } nested(); enum Local { A, B { value: u32 } } let _ = Local::A; let _ = Local::B { value: 1 }; }
impl crate::T { pub fn method(&self) { leaf(); } }
macro_rules! twice { ($f:path) => { $f(); $f(); } }
macro_rules! nested_twice { ($f:path) => { twice!($f); twice!($f); } }
pub fn macros() { nested_twice!(leaf); }
"#).unwrap();
    // Both targets compile the same physical source against a different T.
    let main = root.join("src/main.rs");
    fs::write(&main, format!("pub struct T; {}", fs::read_to_string(&main).unwrap())).unwrap();
}

#[test]
fn shared_source_keeps_each_target_and_ids_survive_offset_shifts() {
    let work = tempfile::tempdir().unwrap();
    write_fixture(work.path());
    let loaded = graph::load(work.path()).unwrap();
    let model = graph::extract_workspace_model(&loaded);
    assert!(model.usages.iter().all(|usage| model.nodes.contains_key(&usage.target)));
    let targets: Vec<_> = model.nodes.values().filter(|n| n.kind == NodeKind::Crate).collect();
    assert_eq!(targets.len(), 2);
    let mut before = BTreeMap::new();
    for target in targets {
        let kind = target.crate_target_kind.as_deref().unwrap();
        let item = |name: &str| model.nodes.values().find(|n| n.crate_id == Some(target.id) && n.qualified_name == name).unwrap();
        let run = item("identity::shared::run");
        let leaf = item("identity::shared::leaf");
        assert_eq!(item("identity::shared::run::Local::A").item_kind, Some(graph::ItemKind::EnumVariant));
        assert!(model.usages.iter().any(|u| u.target == leaf.id && u.consumer_item == Some(run.id)), "shared-source call missing in {kind}");
        let macros = item("identity::shared::macros");
        let calls: Vec<_> = model.usages.iter().filter(|u| u.consumer_item == Some(macros.id) && u.target == leaf.id).collect();
        assert_eq!(calls.len(), 4, "two nested invocations expand to four calls in {kind}");
        assert_eq!(calls.iter().map(|u| (&u.file, u.start, u.end)).collect::<HashSet<_>>().len(), 1);
        assert_eq!(calls.iter().map(|u| serde_json::to_string(&u.expansion).unwrap()).collect::<HashSet<_>>().len(), 4);
        for node in model.nodes.values().filter(|n| n.crate_id == Some(target.id)) {
            before.insert((kind.to_owned(), node.qualified_name.clone(), format!("{:?}", node.kind)), node.id);
        }
    }
    drop(loaded);
    for file in ["src/lib.rs", "src/main.rs", "src/shared.rs"] {
        let path = work.path().join(file);
        fs::write(&path, format!("// inserted before every declaration: é\n\n{}", fs::read_to_string(&path).unwrap())).unwrap();
    }
    let loaded = graph::load(work.path()).unwrap();
    let after = graph::extract_workspace_model(&loaded);
    for node in after.nodes.values().filter(|n| n.crate_id.is_some()) {
        let target = &after.nodes[&node.crate_id.unwrap()];
        let key = (target.crate_target_kind.clone().unwrap(), node.qualified_name.clone(), format!("{:?}", node.kind));
        assert_eq!(before.get(&key), Some(&node.id), "identity shifted for {key:?}");
    }
    assert_eq!(before.len(), after.nodes.values().filter(|n| n.crate_id.is_some()).count());
}
