use crate::policy::keys::simple_hash;
use std::path::Path;

/// Regular files only (a symlink could point at `/dev/zero` or outside the
/// worktree), and at most the first 1 MiB plus the length, so a huge
/// untracked artifact can't stall the daemon.
fn untracked_fingerprint(path: &Path) -> String {
    use std::io::Read;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => {
            let mut head = Vec::new();
            if let Ok(file) = std::fs::File::open(path) {
                let _ = file.take(1 << 20).read_to_end(&mut head);
            }
            format!("{}:{}", meta.len(), String::from_utf8_lossy(&head))
        }
        Ok(meta) => format!("{:?}", meta.file_type()),
        Err(_) => String::new(),
    }
}

pub(crate) fn diff_hash(worktree: &Path, base: &str) -> String {
    let diff = crate::git::changes::diff_full(worktree, base, usize::MAX).unwrap_or_default();
    let untracked = crate::git::changes::status_porcelain(worktree).unwrap_or_default();
    // The base is part of the key: after the work is carried onto a moved
    // base, `git diff` of the task's own files can be byte-identical while
    // the tree that would land is not.
    let mut input = format!("{base}\u{0}{diff}");
    input.push_str("\u{0}untracked\u{0}");
    // Contents too: `git diff` never shows an untracked file, so hashing
    // only its name reused a stale failing result after the agent fixed a
    // file it had created.
    for u in &untracked {
        input.push_str(u);
        input.push('\u{0}');
        input.push_str(&untracked_fingerprint(&worktree.join(u)));
        input.push('\u{0}');
    }
    simple_hash(&input)
}
