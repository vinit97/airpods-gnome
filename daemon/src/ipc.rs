// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static STATUS_TEMP_ID: AtomicU64 = AtomicU64::new(0);

pub fn socket_path() -> io::Result<PathBuf> {
    let dir = env::var_os("XDG_RUNTIME_DIR")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is unset"))?;
    let dir = PathBuf::from(dir);
    if !dir.is_absolute() {
        return Err(io::Error::other("XDG_RUNTIME_DIR must be absolute"));
    }
    Ok(dir.join("airpods-gnome.sock"))
}

pub fn user_dir(variable: &str, fallback: &str) -> io::Result<PathBuf> {
    if let Some(value) = env::var_os(variable).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(fallback))
        .ok_or_else(|| io::Error::other("HOME is unset"))
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

/// Atomically replace ephemeral status. Saved settings use their own durable writer;
/// this snapshot is rebuilt on startup and does not need a disk flush per update.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = path.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        STATUS_TEMP_ID.fetch_add(1, Ordering::Relaxed)
    ));
    atomic_write_with_temp(path, &temp, bytes)
}

fn atomic_write_with_temp(path: &Path, temp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(temp)?;
    let result = file.write_all(bytes).and_then(|()| fs::rename(temp, path));
    if result.is_err() {
        // Only remove a temporary file created by this call, never a collision.
        let _ = fs::remove_file(temp);
    }
    result
}

/// An advisory lock outlives the socket and also covers stale-socket cleanup.
pub fn daemon_lock(socket: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(socket.with_extension("lock"))?;
    file.try_lock()
        .map_err(|_| io::Error::other("AirPods backend is already running"))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::unix::fs::symlink;

    #[test]
    fn status_replacement_is_atomic_and_private() {
        let directory = tempfile::tempdir().unwrap();
        let status = directory.path().join("status.json");
        fs::write(&status, b"old status").unwrap();
        fs::set_permissions(&status, fs::Permissions::from_mode(0o644)).unwrap();
        let mut previous = File::open(&status).unwrap();

        atomic_write(&status, b"new status").unwrap();

        let mut old_contents = String::new();
        previous.read_to_string(&mut old_contents).unwrap();
        assert_eq!(old_contents, "old status");
        assert_eq!(fs::read(&status).unwrap(), b"new status");
        assert_eq!(
            fs::metadata(&status).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn planted_legacy_temporary_links_preserve_their_targets() {
        for symbolic in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let status = directory.path().join("status.json");
            let temporary = status.with_extension("tmp");
            let victim = directory.path().join("unrelated-file");
            fs::write(&victim, b"keep these contents").unwrap();
            fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
            if symbolic {
                symlink(&victim, &temporary).unwrap();
            } else {
                fs::hard_link(&victim, &temporary).unwrap();
            }

            atomic_write(&status, b"new status").unwrap();

            assert_eq!(fs::read(&victim).unwrap(), b"keep these contents");
            assert_eq!(
                fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
                0o644
            );
            assert_eq!(fs::read(&temporary).unwrap(), b"keep these contents");
            assert_eq!(fs::read(&status).unwrap(), b"new status");
            assert!(fs::symlink_metadata(&status).unwrap().file_type().is_file());
        }
    }

    #[test]
    fn temporary_collisions_are_not_followed_modified_or_removed() {
        for kind in ["file", "symlink", "hardlink"] {
            let directory = tempfile::tempdir().unwrap();
            let status = directory.path().join("status.json");
            let temporary = directory.path().join("colliding.tmp");
            let victim = directory.path().join("unrelated-file");
            fs::write(&status, b"old status").unwrap();
            fs::write(&victim, b"keep these contents").unwrap();
            fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
            match kind {
                "file" => fs::write(&temporary, b"keep these contents").unwrap(),
                "symlink" => symlink(&victim, &temporary).unwrap(),
                "hardlink" => fs::hard_link(&victim, &temporary).unwrap(),
                _ => unreachable!(),
            }

            let error = atomic_write_with_temp(&status, &temporary, b"new status").unwrap_err();

            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
            assert_eq!(fs::read(&status).unwrap(), b"old status");
            assert_eq!(fs::read(&temporary).unwrap(), b"keep these contents");
            assert_eq!(fs::read(&victim).unwrap(), b"keep these contents");
            assert_eq!(
                fs::metadata(&victim).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
    }

    #[test]
    fn failed_rename_removes_the_new_temporary_file() {
        let directory = tempfile::tempdir().unwrap();
        let status = directory.path().join("status.json");
        fs::create_dir(&status).unwrap();
        fs::write(status.join("existing-entry"), b"keep these contents").unwrap();

        assert!(atomic_write(&status, b"new status").is_err());

        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        assert_eq!(
            fs::read(status.join("existing-entry")).unwrap(),
            b"keep these contents"
        );
    }

    #[test]
    fn existing_status_symlink_is_replaced_without_touching_its_target() {
        let directory = tempfile::tempdir().unwrap();
        let status = directory.path().join("status.json");
        let victim = directory.path().join("unrelated-file");
        fs::write(&victim, b"keep these contents").unwrap();
        symlink(&victim, &status).unwrap();

        atomic_write(&status, b"new status").unwrap();

        assert_eq!(fs::read(&victim).unwrap(), b"keep these contents");
        assert_eq!(fs::read(&status).unwrap(), b"new status");
        assert!(fs::symlink_metadata(&status).unwrap().file_type().is_file());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
