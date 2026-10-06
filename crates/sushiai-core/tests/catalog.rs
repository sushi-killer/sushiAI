use sushiai_core::catalog::{project_for_cwd, Catalog, CatalogError};
use sushiai_protocol::catalog::{Group, GroupsSync, Project, ProjectFolder, ProjectsSync};

const HOST: &str = "devbox";

fn project(id: &str, rev: u64, at: u64, folders: &[(&str, &str)]) -> Project {
    Project {
        id: id.into(),
        name: format!("name-{id}"),
        folders: folders
            .iter()
            .map(|(h, p)| ProjectFolder {
                host: (*h).into(),
                path: (*p).into(),
            })
            .collect(),
        rev,
        updated_at: at,
        deleted: false,
    }
}

fn group(id: &str, project: &str, rev: u64, at: u64) -> Group {
    Group {
        id: id.into(),
        project_id: project.into(),
        name: format!("g-{id}"),
        order: 0,
        rev,
        updated_at: at,
        deleted: false,
    }
}

fn psync(projects: Vec<Project>, full: bool) -> ProjectsSync {
    ProjectsSync {
        host: HOST.into(),
        projects,
        full,
    }
}

fn gsync(groups: Vec<Group>, full: bool) -> GroupsSync {
    GroupsSync { groups, full }
}

fn local(id: &str, rev: u64, at: u64) -> Project {
    project(id, rev, at, &[(HOST, "/home/user/a")])
}

#[test]
fn higher_rev_wins_lower_ignored() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 2, 10)], false), HOST);
    let r = c.apply_projects(&psync(vec![local("p1", 1, 99)], false), HOST);
    assert_eq!((r.applied, r.ignored, r.rev), (0, 1, 2));
    let r = c.apply_projects(&psync(vec![local("p1", 3, 1)], false), HOST);
    assert_eq!((r.applied, r.ignored, r.rev), (1, 0, 3));
    assert_eq!(c.projects["p1"].rev, 3);
}

#[test]
fn equal_rev_uses_updated_at_and_reapply_is_idempotent() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 2, 10)], false), HOST);
    let r = c.apply_projects(&psync(vec![local("p1", 2, 20)], false), HOST);
    assert_eq!(r.applied, 1);
    let before = c.clone();
    let r = c.apply_projects(&psync(vec![local("p1", 2, 20)], false), HOST);
    assert_eq!((r.applied, r.ignored), (0, 1));
    let r = c.apply_projects(&psync(vec![local("p1", 2, 5)], false), HOST);
    assert_eq!((r.applied, r.ignored), (0, 1));
    assert_eq!(c, before);
}

#[test]
fn full_sync_tombstones_absent_records_without_bumping_rev() {
    let mut c = Catalog::new();
    c.apply_projects(
        &psync(vec![local("p1", 1, 1), local("p2", 1, 1)], false),
        HOST,
    );
    let r = c.apply_projects(&psync(vec![local("p1", 1, 1)], true), HOST);
    assert_eq!(r.applied, 1);
    assert!(c.projects["p2"].deleted);
    assert_eq!(
        c.projects["p2"].rev, 1,
        "a tombstone made by a sync keeps its rev"
    );
    assert!(!c.projects["p1"].deleted);
    // Re-applying the same full payload changes nothing.
    let before = c.clone();
    c.apply_projects(&psync(vec![local("p1", 1, 1)], true), HOST);
    assert_eq!(c, before);
    // A partial sync with a higher rev can still bring it back.
    c.apply_projects(&psync(vec![local("p2", 5, 1)], false), HOST);
    assert!(!c.projects["p2"].deleted);
}

#[test]
fn a_full_sync_replaces_records_whatever_their_rev() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 9, 90)], false), HOST);
    c.apply_groups(&gsync(vec![group("g1", "p1", 9, 90)], false));
    // The desktop was reset: its revs start over, lower than what the daemon holds.
    let mut renamed = local("p1", 1, 1);
    renamed.name = "after-reset".into();
    let r = c.apply_projects(&psync(vec![renamed], true), HOST);
    assert_eq!((r.applied, r.ignored), (1, 0));
    assert_eq!(c.projects["p1"].name, "after-reset");
    assert_eq!(c.projects["p1"].rev, 1);
    let mut regrouped = group("g1", "p1", 1, 1);
    regrouped.name = "after-reset".into();
    let r = c.apply_groups(&gsync(vec![regrouped], true));
    assert_eq!((r.applied, r.ignored), (1, 0));
    assert_eq!(c.groups["g1"].name, "after-reset");
    // The same partial payload with a lower rev is still ignored.
    let r = c.apply_projects(&psync(vec![local("p1", 0, 1)], false), HOST);
    assert_eq!(r.ignored, 1);
}

#[test]
fn a_project_removed_and_added_again_gets_its_groups_back() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 3, 1)], true), HOST);
    c.apply_groups(&gsync(vec![group("g1", "p1", 3, 1)], true));
    // Removed: the project and, with it, its group are tombstones; no rev moved.
    c.apply_projects(&psync(vec![], true), HOST);
    assert!(c.projects["p1"].deleted && c.groups["g1"].deleted);
    assert_eq!((c.projects["p1"].rev, c.groups["g1"].rev), (3, 3));
    // Added again with the same rev: the full sync is authoritative.
    c.apply_projects(&psync(vec![local("p1", 3, 1)], true), HOST);
    assert!(!c.projects["p1"].deleted);
    assert!(c.groups["g1"].deleted, "the group waits for its own sync");
    let r = c.apply_groups(&gsync(vec![group("g1", "p1", 3, 1)], true));
    assert_eq!(r.applied, 1);
    assert!(!c.groups["g1"].deleted);
}

#[test]
fn replica_filter_by_host() {
    let mut c = Catalog::new();
    let other = project("p2", 1, 1, &[("elsewhere", "/home/user/b")]);
    let r = c.apply_projects(&psync(vec![local("p1", 1, 1), other], false), HOST);
    assert_eq!((r.applied, r.ignored), (1, 1));
    assert!(!c.projects.contains_key("p2"));

    // Folder moved to another host: the stored project becomes a tombstone.
    let moved = project("p1", 2, 2, &[("elsewhere", "/home/user/a")]);
    c.apply_projects(&psync(vec![moved], false), HOST);
    assert!(c.projects["p1"].deleted);
    assert_eq!(project_for_cwd(&c, HOST, "/home/user/a"), None);

    // A payload addressed to another host is ignored whole.
    let mut other_host = psync(vec![local("p9", 1, 1)], false);
    other_host.host = "elsewhere".into();
    assert_eq!(c.apply_projects(&other_host, HOST).ignored, 1);
}

#[test]
fn groups_follow_their_project() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 1, 1)], false), HOST);
    let r = c.apply_groups(&gsync(
        vec![group("g1", "p1", 1, 1), group("g2", "px", 1, 1)],
        false,
    ));
    assert_eq!((r.applied, r.ignored), (1, 1));
    assert!(!c.groups.contains_key("g2"));

    // Deleting the project tombstones its groups.
    let mut gone = local("p1", 2, 2);
    gone.deleted = true;
    c.apply_projects(&psync(vec![gone], false), HOST);
    assert!(c.groups["g1"].deleted);
    // A live group for a deleted project is refused.
    let r = c.apply_groups(&gsync(vec![group("g3", "p1", 1, 1)], false));
    assert_eq!(r.ignored, 1);
}

#[test]
fn group_last_writer_and_full_sync() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 1, 1)], false), HOST);
    c.apply_groups(&gsync(
        vec![group("g1", "p1", 2, 5), group("g2", "p1", 1, 1)],
        false,
    ));
    let r = c.apply_groups(&gsync(vec![group("g1", "p1", 1, 9)], false));
    assert_eq!(r.ignored, 1);
    c.apply_groups(&gsync(vec![group("g1", "p1", 2, 5)], true));
    assert!(c.groups["g2"].deleted);
    assert!(!c.groups["g1"].deleted);
}

#[test]
fn cwd_matching_by_components() {
    let mut c = Catalog::new();
    let a = project(
        "pa",
        1,
        1,
        &[(HOST, "/home/user/a/b"), ("elsewhere", "/home/user/z")],
    );
    let nested = project("pn", 1, 1, &[(HOST, "/home/user/a/b/deep/")]);
    c.apply_projects(&psync(vec![a, nested], false), HOST);
    let find = |cwd: &str| project_for_cwd(&c, HOST, cwd);
    assert_eq!(find("/home/user/a/b"), Some("pa".into()));
    assert_eq!(find("/home/user/a/b/"), Some("pa".into()));
    assert_eq!(find("/home/user/a/b/src"), Some("pa".into()));
    assert_eq!(find("/home/user/a/bc"), None);
    assert_eq!(find("/home/user/a"), None);
    assert_eq!(find("/home/user/a/b/deep"), Some("pn".into()));
    assert_eq!(find("/home/user/a/b/deep/x/"), Some("pn".into()));
    // A kept project still answers for its folders on other hosts.
    assert_eq!(
        project_for_cwd(&c, "elsewhere", "/home/user/z"),
        Some("pa".into())
    );
}

#[test]
fn section_round_trip_and_version_gate() {
    let mut c = Catalog::new();
    c.apply_projects(&psync(vec![local("p1", 1, 1)], false), HOST);
    c.apply_groups(&gsync(vec![group("g1", "p1", 1, 1)], false));
    let bytes = c.to_bytes().unwrap();
    assert!(std::str::from_utf8(&bytes)
        .unwrap()
        .contains("\"catalogVersion\": 1"));
    assert_eq!(Catalog::from_bytes(&bytes).unwrap(), c);

    let err = Catalog::from_bytes(br#"{"catalogVersion":2,"projects":[],"groups":[]}"#);
    assert!(matches!(
        err,
        Err(CatalogError::UnsupportedVersion(Some(2)))
    ));
    let err = Catalog::from_bytes(br#"{"projects":[],"groups":[]}"#);
    assert!(matches!(err, Err(CatalogError::UnsupportedVersion(None))));
    assert!(matches!(
        Catalog::from_bytes(b"not json"),
        Err(CatalogError::Invalid(_))
    ));
}
