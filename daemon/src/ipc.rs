// SPDX-License-Identifier: GPL-3.0-or-later
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

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
    let temp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    fs::rename(temp, path)
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
