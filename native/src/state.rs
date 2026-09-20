//! Durable, profile-local Sync cursor and file inventory.
//!
//! The profile JSON contains credentials and a compatibility snapshot. This
//! database is authoritative for Sync progress: each filesystem transition is
//! followed by a FULL-synchronous transaction before another is attempted.
use crate::app::VaultConfig;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub struct Journal {
    connection: Connection,
    known: BTreeMap<String, String>,
    applied_version: i64,
}

fn identity(vault: &VaultConfig) -> Result<String> {
    let bytes = serde_json::to_vec(&(&vault.id, &vault.host, &vault.root))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn journal_path(profile_path: &Path, vault: &VaultConfig) -> Result<PathBuf> {
    let parent = profile_path.parent().context("profile has no parent")?;
    let name = profile_path
        .file_name()
        .context("profile has no filename")?;
    Ok(parent.join(format!(
        "{}.{}.state.sqlite",
        name.to_string_lossy(),
        &identity(vault)?[..24]
    )))
}

impl Journal {
    pub fn open(profile_path: &Path, vault: &mut VaultConfig) -> Result<Self> {
        let path = journal_path(profile_path, vault)?;
        let expected = identity(vault)?;
        if path.exists() {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                bail!("Sync state is not a regular file");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    bail!("Sync state must be mode 0600");
                }
            }
        } else {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            options.open(&path)?;
            fs::File::open(path.parent().context("state has no parent")?)?.sync_all()?;
        }
        let mut connection = Connection::open(&path)?;
        connection.pragma_update(None, "journal_mode", "DELETE")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK (id = 1), identity TEXT NOT NULL, applied_version INTEGER NOT NULL CHECK (applied_version >= 0));
             CREATE TABLE IF NOT EXISTS known (path TEXT PRIMARY KEY, hash TEXT NOT NULL) WITHOUT ROWID;",
        )?;
        let existing: Option<(String, i64)> = connection
            .query_row(
                "SELECT identity, applied_version FROM state WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (known, applied_version) = if let Some((actual, version)) = existing {
            if actual != expected || version < 0 {
                bail!("Sync state does not match this vault or is invalid");
            }
            let mut known = BTreeMap::new();
            let mut statement = connection.prepare("SELECT path, hash FROM known ORDER BY path")?;
            let rows = statement.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                let (path, hash) = row?;
                known.insert(path, hash);
            }
            (known, version)
        } else {
            if vault.applied_version < 0 {
                bail!("negative Sync cursor in profile");
            }
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO state (id, identity, applied_version) VALUES (1, ?1, ?2)",
                params![expected, vault.applied_version],
            )?;
            for (path, hash) in &vault.known {
                transaction.execute(
                    "INSERT INTO known (path, hash) VALUES (?1, ?2)",
                    params![path, hash],
                )?;
            }
            transaction.commit()?;
            (vault.known.clone(), vault.applied_version)
        };
        vault.known = known.clone();
        vault.applied_version = applied_version;
        Ok(Self {
            connection,
            known,
            applied_version,
        })
    }

    pub fn checkpoint(&mut self, vault: &VaultConfig) -> Result<()> {
        if vault.applied_version < self.applied_version {
            bail!("Sync cursor cannot move backwards");
        }
        let transaction = self.connection.transaction()?;
        for path in self.known.keys() {
            if !vault.known.contains_key(path) {
                transaction.execute("DELETE FROM known WHERE path = ?1", [path])?;
            }
        }
        for (path, hash) in &vault.known {
            if self.known.get(path) != Some(hash) {
                transaction.execute(
                    "INSERT INTO known (path, hash) VALUES (?1, ?2) ON CONFLICT(path) DO UPDATE SET hash = excluded.hash",
                    params![path, hash],
                )?;
            }
        }
        transaction.execute(
            "UPDATE state SET applied_version = ?1 WHERE id = 1",
            [vault.applied_version],
        )?;
        transaction
            .commit()
            .context("could not durably checkpoint Sync state")?;
        self.known = vault.known.clone();
        self.applied_version = vault.applied_version;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(root: &Path) -> VaultConfig {
        VaultConfig {
            id: "vault".into(),
            host: "data.example.test".into(),
            salt: String::new(),
            base_key: String::new(),
            root: root.to_path_buf(),
            applied_version: 0,
            known: BTreeMap::new(),
        }
    }

    #[test]
    fn durable_state_overrides_stale_profile_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile.json");
        let mut current = vault(dir.path());
        let mut journal = Journal::open(&profile, &mut current).unwrap();
        current.applied_version = 7;
        current.known.insert("note.md".into(), "abc".into());
        journal.checkpoint(&current).unwrap();
        drop(journal);
        let mut stale = vault(dir.path());
        let reopened = Journal::open(&profile, &mut stale).unwrap();
        assert_eq!(stale.applied_version, 7);
        assert_eq!(stale.known.get("note.md").map(String::as_str), Some("abc"));
        drop(reopened);
        assert!(Journal::open(&profile, &mut stale).is_ok());
    }

    #[test]
    fn refuses_backwards_cursor_and_preserves_committed_state() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile.json");
        let mut current = vault(dir.path());
        let mut journal = Journal::open(&profile, &mut current).unwrap();
        current.applied_version = 3;
        journal.checkpoint(&current).unwrap();
        current.applied_version = 2;
        assert!(journal.checkpoint(&current).is_err());
        drop(journal);
        let mut stale = vault(dir.path());
        Journal::open(&profile, &mut stale).unwrap();
        assert_eq!(stale.applied_version, 3);
    }

    #[test]
    fn corrupt_state_fails_closed_instead_of_reseeding_from_stale_profile() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile.json");
        let mut current = vault(dir.path());
        let journal = Journal::open(&profile, &mut current).unwrap();
        drop(journal);
        fs::write(journal_path(&profile, &current).unwrap(), b"not a database").unwrap();
        let mut stale = vault(dir.path());
        assert!(Journal::open(&profile, &mut stale).is_err());
        assert_eq!(stale.applied_version, 0);
    }

    #[cfg(unix)]
    #[test]
    fn state_symlink_is_rejected_without_following_it() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile.json");
        let mut current = vault(dir.path());
        let destination = dir.path().join("outside");
        fs::write(&destination, b"untouched").unwrap();
        symlink(&destination, journal_path(&profile, &current).unwrap()).unwrap();
        assert!(Journal::open(&profile, &mut current).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"untouched");
    }
}
