use super::target_in_plan_path;
use crate::verify::repository::target_in_tree;
#[test]
fn a_target_is_present_by_path_directory_or_glob() {
    let files: Vec<String> = ["src/a.rs", "docs/guide/x.md"].map(String::from).to_vec();
    assert!(target_in_tree("src/a.rs", &files));
    assert!(target_in_tree("docs/guide", &files));
    assert!(target_in_tree("docs/guide/", &files));
    assert!(target_in_tree("src/*.rs", &files));
    assert!(!target_in_tree("src/b.rs", &files));
    assert!(!target_in_tree("docs/gui", &files));
    assert!(target_in_plan_path("src/new/a.rs", "src/new"));
    assert!(target_in_plan_path("src/new/a.rs", "src/**"));
    assert!(target_in_plan_path("src/new/*.rs", "src/new/a.rs"));
    assert!(!target_in_plan_path("tests/a.rs", "src/new"));
}
