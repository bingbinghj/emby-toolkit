use std::{future::Future, io, path::Path};
use tokio::fs::{File, OpenOptions};

pub(super) async fn restore_file(source: &Path, destination: &Path) -> io::Result<()> {
    let mut source = File::open(source).await?;
    restore_with(destination, |mut output| async move {
        tokio::io::copy(&mut source, &mut output).await?;
        output.sync_all().await
    })
    .await
}

async fn restore_with<F, Fut>(destination: &Path, write: F) -> io::Result<()>
where
    F: FnOnce(File) -> Fut,
    Fut: Future<Output = io::Result<()>>,
{
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "资源恢复目标没有父目录"))?;
    let stage = parent.join(format!(".etk-asset-{}", uuid::Uuid::new_v4().simple()));
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stage)
        .await?;
    let outcome = async {
        write(output).await?;
        // hard_link publishes complete bytes without replacing a concurrent file.
        tokio::fs::hard_link(&stage, destination).await
    }
    .await;
    let cleanup = tokio::fs::remove_file(&stage).await;
    outcome.and(cleanup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn failed_partial_resource_write_keeps_final_missing_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let backup = root.path().join("backup");
        let directory = root.path().join("art");
        tokio::fs::create_dir(&directory).await.unwrap();
        let destination = directory.join("chapter.jpg");
        let original = b"complete original image bytes";
        tokio::fs::write(&backup, original).await.unwrap();

        let error = restore_with(&destination, |mut output| async move {
            output.write_all(b"partial").await?;
            output.sync_all().await?;
            Err(io::Error::from_raw_os_error(28))
        })
        .await
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(28));
        assert_eq!(tokio::fs::read(&backup).await.unwrap(), original);
        assert!(
            !tokio::fs::try_exists(&destination).await.unwrap(),
            "a failed restoration must never expose its partial bytes at the formal image path"
        );
        assert!(
            tokio::fs::read_dir(&directory)
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );

        restore_file(&backup, &destination).await.unwrap();
        assert_eq!(tokio::fs::read(&destination).await.unwrap(), original);
    }

    #[tokio::test]
    async fn concurrent_user_resource_is_not_overwritten_or_reported_as_restored() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("index.bif");
        let user_stage = root.path().join("user-image");
        let final_path = destination.clone();
        let error = restore_with(&destination, |mut output| async move {
            output.write_all(b"complete recovered bif").await?;
            output.sync_all().await?;
            tokio::fs::write(&user_stage, b"new user bif").await?;
            tokio::fs::rename(&user_stage, &final_path).await?;
            Ok(())
        })
        .await
        .expect_err("a concurrently published user image must reject recovery publication");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"new user bif"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn existing_user_resource_remains_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let backup = root.path().join("backup");
        let destination = root.path().join("poster.jpg");
        tokio::fs::write(&backup, b"old image").await.unwrap();
        tokio::fs::write(&destination, b"current user image")
            .await
            .unwrap();
        assert_eq!(
            restore_file(&backup, &destination)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(
            tokio::fs::read(&destination).await.unwrap(),
            b"current user image"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
