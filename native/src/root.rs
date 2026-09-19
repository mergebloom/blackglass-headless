use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

static ACTIVE_ROOTS: OnceLock<Mutex<HashMap<PathBuf, (u64, u64)>>> = OnceLock::new();

fn root_key(root: &Path) -> Result<PathBuf> {
    Ok(
        fs::canonicalize(root.parent().context("vault root has no parent")?)?
            .join(root.file_name().context("vault root has no name")?),
    )
}

#[derive(Serialize, Deserialize)]
struct Ownership {
    vault_id: String,
    profile: PathBuf,
    root_dev: u64,
    root_ino: u64,
}

pub fn verify(root: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(root)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("vault root is not a real directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let key = root_key(root)?;
        if let Some(expected) = ACTIVE_ROOTS
            .get()
            .and_then(|roots| roots.lock().ok()?.get(&key).copied())
            && (meta.dev(), meta.ino()) != expected
        {
            bail!("vault root identity changed during operation");
        }
    }
    Ok(())
}

#[cfg(unix)]
fn component_name(component: &std::ffi::OsStr) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::CString::new(component.as_bytes())?)
}

#[cfg(unix)]
pub fn directory_beneath(root: &Path, relative: &Path, create: bool) -> Result<fs::File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    };
    verify(root)?;
    let mut options = fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut current = options.open(root)?;
    let meta = current.metadata()?;
    let key = root_key(root)?;
    if let Some(expected) = ACTIVE_ROOTS
        .get()
        .and_then(|roots| roots.lock().ok()?.get(&key).copied())
        && (meta.dev(), meta.ino()) != expected
    {
        bail!("vault root changed while opening directory descriptor");
    }
    for part in relative.components() {
        let std::path::Component::Normal(name) = part else {
            bail!("unsafe directory path");
        };
        let name = component_name(name)?;
        if create {
            let created = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
            if created == 0 {
                current.sync_all()?;
            } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let fd = unsafe {
            libc::openat(
                current.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        current = unsafe { fs::File::from_raw_fd(fd) };
    }
    Ok(current)
}

#[cfg(unix)]
pub fn open_regular_beneath(root: &Path, relative: &Path) -> Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let parent = directory_beneath(root, relative.parent().unwrap_or(Path::new("")), false)?;
    let name = component_name(relative.file_name().context("file has no name")?)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let file = unsafe { fs::File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        bail!("path is not a regular file");
    }
    Ok(file)
}

#[cfg(unix)]
pub fn create_file_beneath(root: &Path, relative: &Path) -> Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let parent = directory_beneath(root, relative.parent().unwrap_or(Path::new("")), true)?;
    let name = component_name(relative.file_name().context("file has no name")?)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).context("stale partial file or unsafe path");
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(unix)]
fn same_history_device(root: &Path, history_location: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    if fs::metadata(root)?.dev() != fs::metadata(history_location)?.dev() {
        bail!("vault and sibling history must be on the same filesystem");
    }
    Ok(())
}

pub fn history_dir(root: &Path) -> Result<PathBuf> {
    verify(root)?;
    let parent = root.parent().context("vault root has no parent")?;
    #[cfg(unix)]
    same_history_device(root, parent)?;
    let label = hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()));
    let path = parent.join(format!(".blackglass-headless-{label}.history"));
    if !path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            fs::DirBuilder::new().mode(0o700).create(&path)?;
        }
        #[cfg(not(unix))]
        fs::create_dir(&path)?;
        fs::File::open(parent)?.sync_all()?;
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        bail!("invalid local history directory");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        same_history_device(root, &path)?;
        if meta.permissions().mode() & 0o077 != 0 {
            bail!("local history directory must be owner-only");
        }
    }
    Ok(path)
}

/// Verify the actual opened target directory, not only the vault root: a
/// nested mount can otherwise make preservation fail after an atomic exchange.
pub fn history_for_directory(root: &Path, directory: &fs::File) -> Result<PathBuf> {
    let path = history_dir(root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let history = options.open(&path)?;
        if history.metadata()?.dev() != directory.metadata()?.dev() {
            bail!("vault subdirectory and history are on different filesystems");
        }
    }
    Ok(path)
}

/// Hold an exclusive kernel lock on the enrolled directory inode and bind that
/// directory to one vault/profile outside the synchronized file namespace.
pub fn lock(root: &Path, vault_id: &str, profile: &Path) -> Result<fs::File> {
    verify(root)?;
    // Reject unsupported mountpoint layouts before enrolling or mutating the vault.
    history_dir(root)?;
    let key = root_key(root)?;
    let root = fs::canonicalize(root)?;
    let handle = fs::File::open(&root)?;
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    #[cfg(unix)]
    let handle_meta = handle.metadata()?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        let path_meta = fs::metadata(&root)?;
        if handle_meta.dev() != path_meta.dev() || handle_meta.ino() != path_meta.ino() {
            bail!("vault root changed while opening");
        }
        if unsafe { libc::flock(handle.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            bail!("vault root is already in use");
        }
    }
    let parent = root.parent().context("vault root has no parent")?;
    let profile_parent = fs::canonicalize(profile.parent().context("profile has no parent")?)?;
    let profile_name = profile.file_name().context("profile has no filename")?;
    let expected = Ownership {
        vault_id: vault_id.to_owned(),
        profile: profile_parent.join(profile_name),
        #[cfg(unix)]
        root_dev: handle_meta.dev(),
        #[cfg(unix)]
        root_ino: handle_meta.ino(),
        #[cfg(not(unix))]
        root_dev: 0,
        #[cfg(not(unix))]
        root_ino: 0,
    };
    let marker_name = format!(
        ".blackglass-headless-{}.owner",
        hex::encode(Sha256::digest(root.as_os_str().as_encoded_bytes()))
    );
    let marker = parent.join(marker_name);
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    match options.open(&marker) {
        Ok(mut file) => {
            file.write_all(&serde_json::to_vec(&expected)?)?;
            file.sync_all()?;
            fs::File::open(parent)?.sync_all()?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let marker_meta = fs::symlink_metadata(&marker)?;
            if !marker_meta.is_file() || marker_meta.file_type().is_symlink() {
                bail!("invalid vault ownership marker");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if marker_meta.permissions().mode() & 0o077 != 0 {
                    bail!("vault ownership marker must be owner-only");
                }
            }
            let mut body = Vec::new();
            fs::File::open(&marker)?.take(4096).read_to_end(&mut body)?;
            let actual: Ownership = serde_json::from_slice(&body)?;
            if actual.vault_id != expected.vault_id
                || actual.profile != expected.profile
                || actual.root_dev != expected.root_dev
                || actual.root_ino != expected.root_ino
            {
                bail!("vault root belongs to another vault or profile");
            }
        }
        Err(error) => return Err(error.into()),
    }
    ACTIVE_ROOTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| anyhow::anyhow!("vault root registry poisoned"))?
        .insert(key, (expected.root_dev, expected.root_ino));
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn fifo_named_like_note_is_rejected_without_blocking() {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("blocking.md");
        let name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let start = std::time::Instant::now();
        assert!(open_regular_beneath(dir.path(), Path::new("blocking.md")).is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn different_device_history_is_rejected_before_mutation() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let other = Path::new("/dev/shm");
        if !other.is_dir()
            || fs::metadata(other).unwrap().dev() == fs::metadata(dir.path()).unwrap().dev()
        {
            return;
        }
        assert!(same_history_device(other, dir.path()).is_err());
        let mounted_directory = fs::File::open(other).unwrap();
        assert!(history_for_directory(dir.path(), &mounted_directory).is_err());
    }
    #[test]
    fn rejects_another_profile_on_same_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        fs::create_dir(&root).unwrap();
        let one = dir.path().join("one.json");
        let two = dir.path().join("two.json");
        let handle = lock(&root, "vault-1", &one).unwrap();
        assert!(lock(&root, "vault-1", &two).is_err());
        drop(handle);
        assert!(lock(&root, "vault-1", &two).is_err());
        assert!(lock(&root, "vault-1", &one).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn enrolled_root_cannot_be_replaced() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        let outside = dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        let _handle = lock(&root, "vault-1", &dir.path().join("profile.json")).unwrap();
        fs::rename(&root, dir.path().join("moved")).unwrap();
        symlink(&outside, &root).unwrap();
        assert!(verify(&root).is_err());
        fs::remove_file(&root).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(verify(&root).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn intermediate_symlink_swap_cannot_escape_enrolled_root() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        let outside = dir.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub/note.md"), b"inside").unwrap();
        fs::write(outside.join("note.md"), b"outside").unwrap();
        let _handle = lock(&root, "vault-1", &dir.path().join("profile.json")).unwrap();
        let _checked = crate::sync::local_path(&root, "sub/note.md").unwrap();
        fs::rename(root.join("sub"), root.join("moved")).unwrap();
        symlink(&outside, root.join("sub")).unwrap();
        assert!(open_regular_beneath(&root, Path::new("sub/note.md")).is_err());
        assert!(create_file_beneath(&root, Path::new("sub/new.md")).is_err());
        assert!(crate::sync::inventory(&root).is_err());
        assert_eq!(fs::read(outside.join("note.md")).unwrap(), b"outside");
        assert!(!outside.join("new.md").exists());
    }
}
