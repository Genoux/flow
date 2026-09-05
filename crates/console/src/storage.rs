use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

pub fn read(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

pub fn write(path: &Path, contents: &str) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);

    let target = match std::fs::symlink_metadata(path) {
        Ok(_) => std::fs::canonicalize(path)?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => path.to_owned(),
        Err(err) => return Err(err),
    };
    let parent = target
        .parent()
        .ok_or_else(|| io::Error::other("file has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".flow-save-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &target)?;
        std::fs::File::open(parent)?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn saves_follow_symlinks_and_keep_the_target_private() {
        let root = std::env::temp_dir().join(format!("flow-storage-test-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("actual.toml");
        let link = root.join("config.toml");
        write(&target, "original").unwrap();
        symlink("actual.toml", &link).unwrap();
        write(&link, "updated").unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        assert_eq!(read(&target).unwrap(), "updated");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(read(&root).is_err());
        assert_eq!(read(&root.join("missing")).unwrap(), "");
        std::fs::write(&target, [0xff]).unwrap();
        assert!(read(&target).is_err());
        std::fs::remove_file(&target).unwrap();
        assert!(write(&link, "must not replace dangling link").is_err());
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
        std::fs::remove_dir_all(root).unwrap();
    }
}
