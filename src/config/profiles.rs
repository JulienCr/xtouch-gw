//! Profile store: filesystem-backed YAML profile management with versioned history.
//!
//! Layout:
//! ```text
//! profiles/
//!   default.yaml
//!   default.history/
//!     2026-04-29T14-22-01.yaml
//!   live-show.yaml
//!   live-show.history/
//!   _active.txt
//! ```
//!
//! The "active profile" is mirrored into a separate watched path (e.g.
//! `config.yaml`) that the gateway's existing config watcher monitors. This
//! module never modifies the watcher itself; it just keeps the watched file
//! in sync with the selected profile.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};

const ACTIVE_FILE: &str = "_active.txt";
const DEFAULT_PROFILE: &str = "default";
const DEFAULT_RETENTION: usize = 50;
const NAME_MAX_LEN: usize = 64;

/// Errors that can occur while operating on the profile store.
#[derive(thiserror::Error, Debug)]
pub enum ProfileError {
    #[error("invalid profile name: {0}")]
    InvalidName(String),
    #[error("profile not found: {0}")]
    NotFound(String),
    #[error("profile already exists: {0}")]
    AlreadyExists(String),
    #[error("profile is active and cannot be modified that way: {0}")]
    Active(String),
    #[error("conflicting write: on-disk hash does not match expected hash")]
    ConflictingWrite,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("error: {0}")]
    Other(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, ProfileError>;

/// Metadata about a profile file.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileMeta {
    pub name: String,
    pub mtime_ms: u64,
    pub size_bytes: u64,
    pub content_hash: String,
    pub is_active: bool,
}

/// A single history snapshot entry.
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub timestamp: String,
    pub size_bytes: u64,
    pub content_hash: String,
}

/// Filesystem-backed profile store with bounded history retention.
pub struct ProfileStore {
    root: PathBuf,
    watched_path: PathBuf,
    retention: usize,
}

impl ProfileStore {
    /// Construct a new store. `retention == 0` falls back to the default of 50.
    pub fn new(root: PathBuf, watched_path: PathBuf, retention: usize) -> Self {
        let retention = if retention == 0 {
            DEFAULT_RETENTION
        } else {
            retention
        };
        Self {
            root,
            watched_path,
            retention,
        }
    }

    /// Ensure `root` exists. If no profiles exist yet, seed `default.yaml`
    /// from the current `watched_path` (or empty content if missing) and
    /// mark "default" as active.
    pub fn ensure_initialized(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;

        let any_profile = self.list_profile_names()?.into_iter().next().is_some();
        if !any_profile {
            let seed_body = if self.watched_path.exists() {
                fs::read_to_string(&self.watched_path)?
            } else {
                String::new()
            };
            let yaml_path = self.profile_path(DEFAULT_PROFILE);
            atomic_write(&yaml_path, seed_body.as_bytes())?;
            fs::create_dir_all(self.history_dir(DEFAULT_PROFILE))?;
        }

        if !self.active_path().exists() {
            let names = self.list_profile_names()?;
            let chosen = if names.iter().any(|n| n == DEFAULT_PROFILE) {
                DEFAULT_PROFILE.to_string()
            } else {
                names
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| DEFAULT_PROFILE.to_string())
            };
            atomic_write(&self.active_path(), chosen.as_bytes())?;
            self.mirror_to_watched(&chosen)?;
        }

        Ok(())
    }

    /// List all profiles in the store, sorted by name.
    pub fn list(&self) -> Result<Vec<ProfileMeta>> {
        let active = self.active().ok();
        let mut metas = Vec::new();
        for name in self.list_profile_names()? {
            metas.push(self.meta_for(&name, active.as_deref())?);
        }
        metas.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(metas)
    }

    /// Read a profile's body and meta.
    pub fn read(&self, name: &str) -> Result<(String, ProfileMeta)> {
        validate_name(name)?;
        let path = self.profile_path(name);
        let body = read_to_string_or_not_found(&path, name)?;
        let md = fs::metadata(&path)?;
        let active = self.active().ok();
        let meta = build_meta(name, body.as_bytes(), &md, active.as_deref() == Some(name));
        Ok((body, meta))
    }

    /// Write a profile body. If `expected_hash` is `Some`, fails with
    /// [`ProfileError::ConflictingWrite`] if the on-disk hash differs.
    /// Snapshots existing contents first, then atomically writes, prunes
    /// old snapshots, and mirrors to the watched path if active.
    pub fn write(
        &self,
        name: &str,
        body: &str,
        expected_hash: Option<&str>,
    ) -> Result<ProfileMeta> {
        validate_name(name)?;
        let path = self.profile_path(name);
        let existed = path.exists();

        if existed {
            let on_disk = fs::read(&path)?;
            let on_disk_hash = sha256_hex(&on_disk);
            if let Some(expected) = expected_hash {
                if expected != on_disk_hash {
                    return Err(ProfileError::ConflictingWrite);
                }
            }
            self.snapshot_bytes(name, &on_disk)?;
        } else if expected_hash.is_some() {
            return Err(ProfileError::ConflictingWrite);
        }

        atomic_write(&path, body.as_bytes())?;
        fs::create_dir_all(self.history_dir(name))?;
        self.prune_history(name)?;

        let active = self.active().ok();
        if active.as_deref() == Some(name) {
            self.mirror_to_watched(name)?;
        }
        let md = fs::metadata(&path)?;
        Ok(build_meta(
            name,
            body.as_bytes(),
            &md,
            active.as_deref() == Some(name),
        ))
    }

    /// Create a new profile. Fails if it already exists.
    pub fn create(&self, name: &str, body: &str) -> Result<ProfileMeta> {
        validate_name(name)?;
        if self.profile_path(name).exists() {
            return Err(ProfileError::AlreadyExists(name.to_string()));
        }
        atomic_write(&self.profile_path(name), body.as_bytes())?;
        fs::create_dir_all(self.history_dir(name))?;
        let active = self.active().ok();
        self.meta_for(name, active.as_deref())
    }

    /// Duplicate `source` to `new_name`.
    pub fn duplicate(&self, source: &str, new_name: &str) -> Result<ProfileMeta> {
        validate_name(source)?;
        validate_name(new_name)?;
        if self.profile_path(new_name).exists() {
            return Err(ProfileError::AlreadyExists(new_name.to_string()));
        }
        let body = read_to_string_or_not_found(&self.profile_path(source), source)?;
        self.create(new_name, &body)
    }

    /// Rename a profile and its history dir. Updates `_active.txt` if needed.
    pub fn rename(&self, old: &str, new: &str) -> Result<()> {
        validate_name(old)?;
        validate_name(new)?;
        let old_yaml = self.profile_path(old);
        if !old_yaml.exists() {
            return Err(ProfileError::NotFound(old.to_string()));
        }
        let new_yaml = self.profile_path(new);
        if new_yaml.exists() {
            return Err(ProfileError::AlreadyExists(new.to_string()));
        }
        fs::rename(&old_yaml, &new_yaml)?;

        let old_hist = self.history_dir(old);
        let new_hist = self.history_dir(new);
        if old_hist.exists() {
            if new_hist.exists() {
                return Err(ProfileError::AlreadyExists(format!("{} (history)", new)));
            }
            fs::rename(&old_hist, &new_hist)?;
        }

        if let Ok(active) = self.active() {
            if active == old {
                atomic_write(&self.active_path(), new.as_bytes())?;
            }
        }
        Ok(())
    }

    /// Delete a profile. Refuses if the profile is currently active.
    pub fn delete(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        let path = self.profile_path(name);
        if !path.exists() {
            return Err(ProfileError::NotFound(name.to_string()));
        }
        if let Ok(active) = self.active() {
            if active == name {
                return Err(ProfileError::Active(name.to_string()));
            }
        }
        fs::remove_file(&path)?;
        let hist = self.history_dir(name);
        if hist.exists() {
            fs::remove_dir_all(&hist)?;
        }
        Ok(())
    }

    /// Return the currently active profile name.
    pub fn active(&self) -> Result<String> {
        let path = self.active_path();
        if !path.exists() {
            return Err(ProfileError::NotFound(ACTIVE_FILE.to_string()));
        }
        let raw = fs::read_to_string(&path)?;
        let trimmed = raw.trim().to_string();
        if trimmed.is_empty() {
            return Err(ProfileError::NotFound(ACTIVE_FILE.to_string()));
        }
        Ok(trimmed)
    }

    /// Set the active profile and mirror it to `watched_path`.
    pub fn set_active(&self, name: &str) -> Result<()> {
        validate_name(name)?;
        if !self.profile_path(name).exists() {
            return Err(ProfileError::NotFound(name.to_string()));
        }
        atomic_write(&self.active_path(), name.as_bytes())?;
        self.mirror_to_watched(name)?;
        Ok(())
    }

    /// List history snapshots for `name`, newest first.
    pub fn list_history(&self, name: &str) -> Result<Vec<Snapshot>> {
        validate_name(name)?;
        let dir = self.history_dir(name);
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut snaps = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };
            let bytes = fs::read(&path)?;
            snaps.push(Snapshot {
                timestamp: stem,
                size_bytes: bytes.len() as u64,
                content_hash: sha256_hex(&bytes),
            });
        }
        snaps.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(snaps)
    }

    /// Read the raw body of a specific snapshot.
    pub fn read_snapshot(&self, name: &str, timestamp: &str) -> Result<String> {
        validate_name(name)?;
        validate_timestamp(timestamp)?;
        let path = self.history_dir(name).join(format!("{}.yaml", timestamp));
        read_to_string_or_not_found(&path, &format!("{}.history/{}", name, timestamp))
    }

    /// Restore a snapshot into the live profile. The current contents are
    /// snapshotted first by [`Self::write`], so restore is reversible.
    pub fn restore_snapshot(&self, name: &str, timestamp: &str) -> Result<ProfileMeta> {
        let body = self.read_snapshot(name, timestamp)?;
        self.write(name, &body, None)
    }

    // -------------------- internal helpers --------------------

    fn profile_path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.yaml", name))
    }

    fn history_dir(&self, name: &str) -> PathBuf {
        self.root.join(format!("{}.history", name))
    }

    fn active_path(&self) -> PathBuf {
        self.root.join(ACTIVE_FILE)
    }

    fn list_profile_names(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        if !self.root.exists() {
            return Ok(out);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if validate_name(stem).is_ok() {
                    out.push(stem.to_string());
                }
            }
        }
        out.sort();
        Ok(out)
    }

    fn meta_for(&self, name: &str, active: Option<&str>) -> Result<ProfileMeta> {
        let path = self.profile_path(name);
        let bytes = fs::read(&path)?;
        let md = fs::metadata(&path)?;
        Ok(build_meta(name, &bytes, &md, active == Some(name)))
    }

    fn snapshot_bytes(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.history_dir(name);
        fs::create_dir_all(&dir)?;
        // ISO 8601 with ':' replaced by '-' for filename safety.
        let now = Utc::now().format("%Y-%m-%dT%H-%M-%S%.3f").to_string();
        let mut path = dir.join(format!("{}.yaml", now));
        let mut counter = 0u32;
        while path.exists() {
            counter += 1;
            path = dir.join(format!("{}-{}.yaml", now, counter));
        }
        atomic_write(&path, bytes)?;
        Ok(())
    }

    fn prune_history(&self, name: &str) -> Result<()> {
        let dir = self.history_dir(name);
        if !dir.exists() {
            return Ok(());
        }
        let mut files: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("yaml"))
            .collect();
        if files.len() <= self.retention {
            return Ok(());
        }
        files.sort();
        let to_remove = files.len() - self.retention;
        for path in files.into_iter().take(to_remove) {
            let _ = fs::remove_file(&path);
        }
        Ok(())
    }

    fn mirror_to_watched(&self, name: &str) -> Result<()> {
        let src = self.profile_path(name);
        if !src.exists() {
            return Err(ProfileError::NotFound(name.to_string()));
        }
        if let Some(parent) = self.watched_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let body = fs::read(&src)?;
        atomic_write(&self.watched_path, &body)?;
        Ok(())
    }
}

// -------------------- free functions --------------------

fn build_meta(name: &str, bytes: &[u8], md: &fs::Metadata, is_active: bool) -> ProfileMeta {
    let mtime_ms = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    ProfileMeta {
        name: name.to_string(),
        mtime_ms,
        size_bytes: bytes.len() as u64,
        content_hash: sha256_hex(bytes),
        is_active,
    }
}

fn read_to_string_or_not_found(path: &Path, name: &str) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ProfileError::NotFound(name.to_string()))
        },
        Err(e) => Err(e.into()),
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > NAME_MAX_LEN {
        return Err(ProfileError::InvalidName(name.to_string()));
    }
    let ok = name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if !ok {
        return Err(ProfileError::InvalidName(name.to_string()));
    }
    Ok(())
}

fn validate_timestamp(ts: &str) -> Result<()> {
    if ts.is_empty() || ts.len() > 64 {
        return Err(ProfileError::InvalidName(ts.to_string()));
    }
    let ok = ts
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'T');
    if !ok {
        return Err(ProfileError::InvalidName(ts.to_string()));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(s, "{:02x}", byte);
    }
    s
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.flush()?;
        let _ = f.sync_all();
    }
    if let Err(e) = fs::rename(&tmp, path) {
        if path.exists() {
            let _ = fs::remove_file(path);
            fs::rename(&tmp, path)?;
        } else {
            return Err(e.into());
        }
    }
    Ok(())
}

// -------------------- tests --------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        _tmp: TempDir,
        store: ProfileStore,
        watched: PathBuf,
        root: PathBuf,
    }

    fn fixture(retention: usize, seed: Option<&str>) -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("profiles");
        let watched = tmp.path().join("config.yaml");
        if let Some(s) = seed {
            std::fs::write(&watched, s).unwrap();
        }
        let store = ProfileStore::new(root.clone(), watched.clone(), retention);
        Fixture {
            _tmp: tmp,
            store,
            watched,
            root,
        }
    }

    const SAMPLE_YAML: &str = "midi:\n  input_port: in\n  output_port: out\n";

    #[test]
    fn ensure_initialized_seeds_default_from_watched() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();

        assert!(fx.root.join("default.yaml").exists());
        assert!(fx.root.join("_active.txt").exists());

        let metas = fx.store.list().unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].name, "default");
        assert!(metas[0].is_active);

        let body = std::fs::read_to_string(fx.root.join("default.yaml")).unwrap();
        assert_eq!(body, SAMPLE_YAML);
        assert_eq!(fx.store.active().unwrap(), "default");
    }

    #[test]
    fn ensure_initialized_with_no_watched_creates_empty_default() {
        let fx = fixture(50, None);
        fx.store.ensure_initialized().unwrap();
        let body = std::fs::read_to_string(fx.root.join("default.yaml")).unwrap();
        assert_eq!(body, "");
    }

    #[test]
    fn write_creates_snapshot_and_changes_hash() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();

        let (_, before) = fx.store.read("default").unwrap();
        let new_body = "midi:\n  input_port: x\n  output_port: y\n";
        let after = fx.store.write("default", new_body, None).unwrap();

        assert_ne!(before.content_hash, after.content_hash);

        let history = fx.store.list_history("default").unwrap();
        assert_eq!(history.len(), 1, "expected one snapshot of prior content");
        assert_eq!(history[0].content_hash, before.content_hash);

        let mirrored = std::fs::read_to_string(&fx.watched).unwrap();
        assert_eq!(mirrored, new_body);
    }

    #[test]
    fn retention_prunes_old_snapshots() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();

        for i in 0..60 {
            let body = format!("v: {}\n", i);
            fx.store.write("default", &body, None).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        let history = fx.store.list_history("default").unwrap();
        assert_eq!(history.len(), 50, "retention should cap snapshots at 50");
    }

    #[test]
    fn expected_hash_mismatch_returns_conflicting_write() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();

        let result = fx.store.write("default", "new\n", Some("deadbeef"));
        match result {
            Err(ProfileError::ConflictingWrite) => {},
            other => panic!("expected ConflictingWrite, got {:?}", other),
        }
    }

    #[test]
    fn expected_hash_match_succeeds() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        let (_, meta) = fx.store.read("default").unwrap();
        let res = fx
            .store
            .write("default", "new body\n", Some(&meta.content_hash));
        assert!(res.is_ok());
    }

    #[test]
    fn duplicate_creates_independent_copy() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        fx.store.duplicate("default", "live-show").unwrap();

        let names: Vec<_> = fx
            .store
            .list()
            .unwrap()
            .into_iter()
            .map(|m| m.name)
            .collect();
        assert!(names.contains(&"default".to_string()));
        assert!(names.contains(&"live-show".to_string()));

        fx.store.write("live-show", "different\n", None).unwrap();
        let (def_body, _) = fx.store.read("default").unwrap();
        assert_eq!(def_body, SAMPLE_YAML);
    }

    #[test]
    fn rename_moves_yaml_and_history() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        fx.store.write("default", "v2\n", None).unwrap();
        fx.store.duplicate("default", "tmp").unwrap();
        fx.store.write("tmp", "tmp v2\n", None).unwrap();

        fx.store.rename("tmp", "renamed").unwrap();
        assert!(fx.root.join("renamed.yaml").exists());
        assert!(fx.root.join("renamed.history").is_dir());
        assert!(!fx.root.join("tmp.yaml").exists());
        assert!(!fx.root.join("tmp.history").exists());
    }

    #[test]
    fn rename_active_updates_active_file() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        fx.store.rename("default", "main").unwrap();
        assert_eq!(fx.store.active().unwrap(), "main");
    }

    #[test]
    fn cannot_delete_active() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        match fx.store.delete("default") {
            Err(ProfileError::Active(_)) => {},
            other => panic!("expected Active, got {:?}", other),
        }
    }

    #[test]
    fn delete_non_active_removes_files() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        fx.store.duplicate("default", "scratch").unwrap();
        fx.store.write("scratch", "v2\n", None).unwrap();
        fx.store.delete("scratch").unwrap();
        assert!(!fx.root.join("scratch.yaml").exists());
        assert!(!fx.root.join("scratch.history").exists());
    }

    #[test]
    fn restore_snapshot_reverts_and_creates_new_snapshot() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();

        let (original, _) = fx.store.read("default").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        fx.store.write("default", "OVERWRITTEN\n", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        fx.store.write("default", "OVERWRITTEN_2\n", None).unwrap();

        let history = fx.store.list_history("default").unwrap();
        assert!(history.len() >= 2);
        let oldest = history.last().unwrap().clone();
        let snap_body = fx
            .store
            .read_snapshot("default", &oldest.timestamp)
            .unwrap();
        assert_eq!(snap_body, original);

        let history_before = fx.store.list_history("default").unwrap().len();
        fx.store
            .restore_snapshot("default", &oldest.timestamp)
            .unwrap();

        let (restored_body, _) = fx.store.read("default").unwrap();
        assert_eq!(restored_body, original);

        let history_after = fx.store.list_history("default").unwrap().len();
        assert_eq!(
            history_after,
            history_before + 1,
            "restore should snapshot the overwritten state"
        );
    }

    #[test]
    fn invalid_name_rejected() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        for bad in ["", "../etc", "with space", "a/b", "."] {
            let res = fx.store.create(bad, "x");
            match res {
                Err(ProfileError::InvalidName(_)) => {},
                other => panic!("expected InvalidName for {:?}, got {:?}", bad, other),
            }
        }
    }

    #[test]
    fn set_active_mirrors_to_watched() {
        let fx = fixture(50, Some(SAMPLE_YAML));
        fx.store.ensure_initialized().unwrap();
        fx.store.create("live-show", "live: true\n").unwrap();
        fx.store.set_active("live-show").unwrap();
        assert_eq!(fx.store.active().unwrap(), "live-show");
        let mirrored = std::fs::read_to_string(&fx.watched).unwrap();
        assert_eq!(mirrored, "live: true\n");
    }
}
