use std::path::Path;

pub(crate) async fn carry(
    target: &Path,
    sibling: &Path,
    base_sha: &str,
) -> Result<(), crate::git::GitError> {
    let (target, sibling, base) = (
        target.to_path_buf(),
        sibling.to_path_buf(),
        base_sha.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        let mine = crate::git::changes::patch_since(&target, &base)?;
        let theirs = crate::git::changes::patch_since(&sibling, &base)?;
        if let Err(error) = crate::git::changes::replace_work_with_patch(&target, &base, &theirs) {
            let _ = crate::git::changes::replace_work_with_patch(&target, &base, &mine);
            return Err(error);
        }
        Ok(())
    })
    .await
    .unwrap_or_else(|error| Err(crate::git::GitError(error.to_string())))
}
