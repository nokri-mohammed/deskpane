//! Reads and rewrites a JSON file someone cares about (for example
//! `~/.claude/settings.json`) safely: a backup copy that never overwrites an
//! older one, and an atomic write (temp file in the same folder, then rename)
//! that refuses when the file changed since it was read. New files are
//! created private (0600 on unix) and then get the original's permissions; a
//! symlinked file is followed, so the link stays a link.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use sha2::{Digest, Sha256};

// most numbered backup names tried for one second (see backup_file)
const MAX_BACKUP_ATTEMPTS: u32 = 100;

/// Everything that tells two versions of a file apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fingerprint {
    // modified time, None when the OS gives none
    pub modified: Option<SystemTime>,
    // length in bytes
    pub len: u64,
    // SHA-256 of the bytes
    pub sha256: [u8; 32],
}

/// What a file looked like when it was read, to spot a change before writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStamp {
    // there was no file
    Missing,
    // there was a file with this fingerprint
    Present(Fingerprint),
}

/// A JSON file as read: its value (None when there was no file) and stamp.
#[derive(Debug, Clone)]
pub struct LoadedJson {
    pub value: Option<Value>,
    pub stamp: FileStamp,
}

/*
 * the real file behind path: a symlink is resolved, so the target is read,
 * backed up and replaced while the link itself stays. A path that does not
 * exist yet is used as given
 */
fn resolve_target(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path);
    match resolved {
        Ok(real) => real,
        Err(_) => path.to_path_buf(),
    }
}

/*
 * the bytes of the file at path and their stamp
 *
 * returns: (None, Missing) when there is no file, else the bytes and a
 *          fingerprint of exactly those bytes; Err when it cannot be read
 */
fn read_with_stamp(path: &Path) -> Result<(Option<Vec<u8>>, FileStamp), String> {
    let shown = path.display();

    // the modified time first: a write after this point changes the fingerprint
    let metadata = std::fs::metadata(path);
    let modified = match metadata {
        Ok(m) => {
            let time = m.modified();
            match time {
                Ok(t) => Some(t),
                Err(_) => None,
            }
        }
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok((None, FileStamp::Missing));
            }
            return Err(format!("cannot read {shown}: {e}"));
        }
    };

    let read = std::fs::read(path);
    let bytes = match read {
        Ok(b) => b,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok((None, FileStamp::Missing));
            }
            return Err(format!("cannot read {shown}: {e}"));
        }
    };

    let digest = Sha256::digest(&bytes);
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(&digest);
    let fingerprint = Fingerprint {
        modified: modified,
        len: bytes.len() as u64,
        sha256: sha256,
    };
    Ok((Some(bytes), FileStamp::Present(fingerprint)))
}

// a file next to path, named after it plus a suffix, e.g. settings.json.x
fn sibling(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let file_name_found = path.file_name();
    let file_name = match file_name_found {
        Some(n) => n,
        None => {
            let shown = path.display();
            return Err(format!("{shown} is not a file path"));
        }
    };
    let mut name = file_name.to_os_string();
    name.push(suffix);
    Ok(path.with_file_name(name))
}

/*
 * creates a new file that only its owner can read (0600 on unix). create_new
 * fails on an existing name, and never follows a symlink planted there
 */
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut options = File::options();
    options.write(true);
    options.create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

// gives copy the permissions of original; a failure is an error
fn copy_permissions(original: &Path, copy: &Path) -> Result<(), String> {
    let metadata = std::fs::metadata(original);
    let permissions = match metadata {
        Ok(m) => m.permissions(),
        Err(e) => {
            let shown = original.display();
            return Err(format!("cannot read the permissions of {shown}: {e}"));
        }
    };

    let set = std::fs::set_permissions(copy, permissions);
    match set {
        Ok(_) => Ok(()),
        Err(e) => {
            let shown = copy.display();
            Err(format!("cannot set the permissions of {shown}: {e}"))
        }
    }
}

// removes a file and ignores any error (cleanup on a failure path)
fn remove_quietly(path: &Path) {
    let removed = std::fs::remove_file(path);
    match removed {
        Ok(_) => {}
        Err(_) => {}
    }
}

/*
 * reads and parses a JSON file (through a symlink, if it is one)
 *
 * path: the file
 * returns: its value and stamp; value None and stamp Missing when there is
 *          no file; Err when it cannot be read or is not valid JSON
 */
pub fn read_json_file(path: &Path) -> Result<LoadedJson, String> {
    let target = resolve_target(path);
    let read = read_with_stamp(&target);
    let (bytes, stamp) = match read {
        Ok(pair) => pair,
        Err(reason) => return Err(reason),
    };

    let content = match bytes {
        Some(b) => b,
        None => {
            let missing = LoadedJson {
                value: None,
                stamp: FileStamp::Missing,
            };
            return Ok(missing);
        }
    };

    let parsed = serde_json::from_slice::<Value>(&content);
    let value = match parsed {
        Ok(v) => v,
        Err(e) => {
            let shown = path.display();
            return Err(format!("{shown} is not valid JSON ({e})"));
        }
    };

    let loaded = LoadedJson {
        value: Some(value),
        stamp: stamp,
    };
    Ok(loaded)
}

/*
 * copies a file to <name>.deskpane-backup-<suffix> next to it (next to the
 * real file when path is a symlink), and never overwrites a file that is
 * already there (a taken name gets -1, -2, ...). The backup is created
 * private, then gets the original's permissions.
 *
 * path: the file to back up
 * suffix: e.g. 20261108-154000
 * returns: the backup's path, or Err (nothing is left behind on an error)
 */
pub fn backup_file(path: &Path, suffix: &str) -> Result<PathBuf, String> {
    let target = resolve_target(path);
    let shown = target.display();

    // open the original first, so a missing one leaves no empty backup
    let opened = File::open(&target);
    let mut original = match opened {
        Ok(f) => f,
        Err(e) => return Err(format!("cannot read {shown}: {e}")),
    };

    // a taken name (a backup from the same second) moves on to the next number
    let mut found: Option<(PathBuf, File)> = None;
    for attempt in 0..MAX_BACKUP_ATTEMPTS {
        let name;
        if attempt == 0 {
            name = format!(".deskpane-backup-{suffix}");
        } else {
            name = format!(".deskpane-backup-{suffix}-{attempt}");
        }
        let named = sibling(&target, &name);
        let candidate = match named {
            Ok(b) => b,
            Err(reason) => return Err(reason),
        };

        let created = create_private(&candidate);
        match created {
            Ok(f) => {
                found = Some((candidate, f));
                break;
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    continue;
                }
                let candidate_shown = candidate.display();
                return Err(format!("cannot create backup {candidate_shown}: {e}"));
            }
        }
    }
    let (backup, mut copy) = match found {
        Some(pair) => pair,
        None => {
            return Err(format!(
                "cannot create a backup of {shown}: too many backups"
            ))
        }
    };
    let backup_shown = backup.display();

    let copied = std::io::copy(&mut original, &mut copy);
    match copied {
        Ok(_) => {}
        Err(e) => {
            drop(copy);
            remove_quietly(&backup);
            return Err(format!("cannot write backup {backup_shown}: {e}"));
        }
    }

    let synced = copy.sync_all();
    match synced {
        Ok(_) => {}
        Err(e) => {
            drop(copy);
            remove_quietly(&backup);
            return Err(format!("cannot write backup {backup_shown}: {e}"));
        }
    }
    drop(copy);

    // the same permissions as the original, never wider by accident
    let permissions_copied = copy_permissions(&target, &backup);
    match permissions_copied {
        Ok(()) => Ok(backup),
        Err(reason) => {
            remove_quietly(&backup);
            Err(reason)
        }
    }
}

/*
 * creates the private temp file next to target. A file already at that name
 * (left by a crashed run, or a planted symlink) is removed once, never written
 * through
 */
fn create_temp(temp: &Path) -> Result<File, String> {
    let first = create_private(temp);
    match first {
        Ok(f) => return Ok(f),
        Err(e) => {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                let shown = temp.display();
                return Err(format!("cannot create {shown}: {e}"));
            }
        }
    }

    remove_quietly(temp);
    let second = create_private(temp);
    match second {
        Ok(f) => Ok(f),
        Err(e) => {
            let shown = temp.display();
            Err(format!("cannot create {shown}: {e}"))
        }
    }
}

/*
 * writes bytes to a private temp file in the same folder, checks the target
 * still has the fingerprint it had when it was read, then renames the temp
 * file over it. A symlinked path updates the real file and keeps the link.
 *
 * path: the file to replace (its folder is created if missing)
 * bytes: the new content
 * stamp: what read_json_file saw; a different stamp now (time, length or
 *        bytes) means someone else wrote the file, so nothing is written
 * returns: Ok(()) or Err with the reason; the temp file never stays behind
 */
pub fn write_atomically(path: &Path, bytes: &[u8], stamp: FileStamp) -> Result<(), String> {
    let target = resolve_target(path);
    let shown = target.display();

    // the folder, e.g. ~/.claude on a machine where Claude Code never wrote settings
    let parent_found = target.parent();
    let parent = match parent_found {
        Some(p) => p,
        None => Path::new(""),
    };
    if !parent.as_os_str().is_empty() {
        let made = std::fs::create_dir_all(parent);
        match made {
            Ok(_) => {}
            Err(e) => {
                let parent_shown = parent.display();
                return Err(format!("cannot create {parent_shown}: {e}"));
            }
        }
    }

    let pid = std::process::id();
    let named = sibling(&target, &format!(".deskpane-tmp-{pid}"));
    let temp = match named {
        Ok(t) => t,
        Err(reason) => return Err(reason),
    };

    let created = create_temp(&temp);
    let mut file = match created {
        Ok(f) => f,
        Err(reason) => return Err(reason),
    };

    // write and flush to disk before it replaces the original
    let written = file.write_all(bytes);
    match written {
        Ok(_) => {}
        Err(e) => {
            drop(file);
            remove_quietly(&temp);
            return Err(format!("cannot write next to {shown}: {e}"));
        }
    }
    let synced = file.sync_all();
    match synced {
        Ok(_) => {}
        Err(e) => {
            drop(file);
            remove_quietly(&temp);
            return Err(format!("cannot write next to {shown}: {e}"));
        }
    }
    drop(file);

    // someone else wrote (or removed) the file since it was read: stop
    let now_read = read_with_stamp(&target);
    let now_stamp = match now_read {
        Ok((_, s)) => s,
        Err(reason) => {
            remove_quietly(&temp);
            return Err(reason);
        }
    };
    if now_stamp != stamp {
        remove_quietly(&temp);
        return Err(format!(
            "{shown} changed while deskpane was editing it (Claude Code may have written it). Nothing was changed; run the command again"
        ));
    }

    // the new file gets the old one's permissions (settings.json may be 0600)
    match stamp {
        FileStamp::Present(_) => {
            let permissions_copied = copy_permissions(&target, &temp);
            match permissions_copied {
                Ok(()) => {}
                Err(reason) => {
                    remove_quietly(&temp);
                    return Err(reason);
                }
            }
        }
        FileStamp::Missing => {}
    }

    let renamed = std::fs::rename(&temp, &target);
    match renamed {
        Ok(_) => Ok(()),
        Err(e) => {
            remove_quietly(&temp);
            Err(format!("cannot replace {shown}: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::Duration;

    // a fresh, empty folder for one test: short and unique
    fn test_dir(name: &str) -> PathBuf {
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("dp-test-{pid}-{name}"));
        let removed = fs::remove_dir_all(&dir);
        match removed {
            Ok(_) => {}
            Err(_) => {}
        }
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // the names of the files in a folder, sorted
    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            names.push(entry.file_name().to_string_lossy().to_string());
        }
        names.sort();
        names
    }

    // ---- read ----

    #[test]
    fn missing_file_reads_as_none() {
        let dir = test_dir("read-missing");

        let loaded = read_json_file(&dir.join("settings.json")).unwrap();

        assert!(loaded.value.is_none());
        assert_eq!(loaded.stamp, FileStamp::Missing);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn valid_file_reads_with_its_stamp() {
        let dir = test_dir("read-valid");
        let path = dir.join("settings.json");
        fs::write(&path, br#"{"model":"opus"}"#).unwrap();

        let loaded = read_json_file(&path).unwrap();

        assert_eq!(loaded.value, Some(serde_json::json!({"model": "opus"})));
        match loaded.stamp {
            FileStamp::Present(_) => {}
            FileStamp::Missing => panic!("expected a present stamp"),
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = test_dir("read-invalid");
        let path = dir.join("settings.json");
        fs::write(&path, b"{\"hooks\": {,}").unwrap();

        let read = read_json_file(&path);

        match read {
            Ok(_) => panic!("expected an error"),
            Err(reason) => assert!(reason.contains("not valid JSON"), "{reason}"),
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_file_is_an_error() {
        let dir = test_dir("read-empty");
        let path = dir.join("settings.json");
        fs::write(&path, b"").unwrap();

        assert!(read_json_file(&path).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- backup ----

    #[test]
    fn backup_copies_the_bytes_next_to_the_file() {
        let dir = test_dir("backup-copy");
        let path = dir.join("settings.json");
        fs::write(&path, b"{\"a\": 1}\n").unwrap();

        let backup = backup_file(&path, "20261108-154000").unwrap();

        assert_eq!(
            backup,
            dir.join("settings.json.deskpane-backup-20261108-154000")
        );
        assert_eq!(fs::read(&backup).unwrap(), b"{\"a\": 1}\n");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_never_overwrites_an_older_backup() {
        // two backups in the same second: the second one gets a numbered name
        let dir = test_dir("backup-twice");
        let path = dir.join("settings.json");
        fs::write(&path, b"first").unwrap();
        backup_file(&path, "20261108-154000").unwrap();
        fs::write(&path, b"second").unwrap();

        let again = backup_file(&path, "20261108-154000").unwrap();

        let kept = dir.join("settings.json.deskpane-backup-20261108-154000");
        assert_eq!(fs::read(&kept).unwrap(), b"first");
        assert_eq!(
            again,
            dir.join("settings.json.deskpane-backup-20261108-154000-1")
        );
        assert_eq!(fs::read(&again).unwrap(), b"second");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn backup_of_a_missing_file_is_an_error() {
        let dir = test_dir("backup-missing");

        let backup = backup_file(&dir.join("settings.json"), "x");

        assert!(backup.is_err());
        assert!(files_in(&dir).is_empty(), "no empty backup is left behind");
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- atomic write ----

    #[test]
    fn write_creates_a_missing_file_and_its_folder() {
        let dir = test_dir("write-new");
        let path = dir.join(".claude").join("settings.json");

        let written = write_atomically(&path, b"{}\n", FileStamp::Missing);

        assert_eq!(written, Ok(()));
        assert_eq!(fs::read(&path).unwrap(), b"{}\n");
        assert_eq!(files_in(&dir.join(".claude")), vec!["settings.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_replaces_an_unchanged_file_and_leaves_no_temp_file() {
        let dir = test_dir("write-replace");
        let path = dir.join("settings.json");
        fs::write(&path, b"{\"old\": true}").unwrap();
        let loaded = read_json_file(&path).unwrap();

        let written = write_atomically(&path, b"{\"new\": true}", loaded.stamp);

        assert_eq!(written, Ok(()));
        assert_eq!(fs::read(&path).unwrap(), b"{\"new\": true}");
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_refuses_when_the_file_changed_since_it_was_read() {
        let dir = test_dir("write-changed");
        let path = dir.join("settings.json");
        fs::write(&path, b"{}").unwrap();
        let loaded = read_json_file(&path).unwrap();

        // someone else (Claude Code) writes the file in between
        fs::write(&path, b"{\"theirs\": 1}").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        drop(file);

        let written = write_atomically(&path, b"{\"ours\": 1}", loaded.stamp);

        assert!(written.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{\"theirs\": 1}");
        assert_eq!(files_in(&dir), vec!["settings.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_refuses_when_a_missing_file_appeared() {
        let dir = test_dir("write-appeared");
        let path = dir.join("settings.json");
        fs::write(&path, b"{\"theirs\": 1}").unwrap();

        let written = write_atomically(&path, b"{}", FileStamp::Missing);

        assert!(written.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{\"theirs\": 1}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_refuses_when_the_file_was_deleted() {
        let dir = test_dir("write-deleted");
        let path = dir.join("settings.json");
        fs::write(&path, b"{}").unwrap();
        let loaded = read_json_file(&path).unwrap();
        fs::remove_file(&path).unwrap();

        let written = write_atomically(&path, b"{}", loaded.stamp);

        assert!(written.is_err());
        assert!(files_in(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_keeps_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("write-mode");
        let path = dir.join("settings.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let loaded = read_json_file(&path).unwrap();

        write_atomically(&path, b"{\"a\":1}", loaded.stamp).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- review fixes (T-045) ----

    #[cfg(unix)]
    #[test]
    fn backup_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("backup-mode");
        let path = dir.join("settings.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let backup = backup_file(&path, "20261108-154000").unwrap();

        let mode = fs::metadata(&backup).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn temp_file_is_never_world_readable() {
        // a new file is the temp file renamed, so its mode is the temp file's mode
        use std::os::unix::fs::PermissionsExt;

        let dir = test_dir("temp-mode");
        let path = dir.join("settings.json");

        write_atomically(&path, b"{}", FileStamp::Missing).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stale_temp_symlink_is_not_followed() {
        // a link planted at the temp file's name must not redirect the write
        let dir = test_dir("temp-link");
        let path = dir.join("settings.json");
        let victim = dir.join("victim.txt");
        fs::write(&victim, b"keep me").unwrap();
        let pid = std::process::id();
        let temp = dir.join(format!("settings.json.deskpane-tmp-{pid}"));
        std::os::unix::fs::symlink(&victim, &temp).unwrap();

        write_atomically(&path, b"{\"a\":1}", FileStamp::Missing).unwrap();

        assert_eq!(fs::read(&victim).unwrap(), b"keep me");
        assert_eq!(fs::read(&path).unwrap(), b"{\"a\":1}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn write_through_a_symlink_updates_the_target_and_keeps_the_link() {
        // dotfile setups often link ~/.claude/settings.json into a repo
        let dir = test_dir("write-link");
        fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let target = dir.join("dotfiles").join("settings.json");
        fs::write(&target, b"{\"old\":1}").unwrap();
        let link = dir.join("settings.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let loaded = read_json_file(&link).unwrap();
        let backup = backup_file(&link, "20261108-154000").unwrap();
        write_atomically(&link, b"{\"new\":1}", loaded.stamp).unwrap();

        let link_meta = fs::symlink_metadata(&link).unwrap();
        assert!(link_meta.file_type().is_symlink(), "the link stays a link");
        assert_eq!(fs::read(&target).unwrap(), b"{\"new\":1}");
        assert_eq!(fs::read(&backup).unwrap(), b"{\"old\":1}");
        assert_eq!(
            backup.parent().unwrap(),
            dir.join("dotfiles").canonicalize().unwrap()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn change_with_the_same_mtime_is_detected() {
        let dir = test_dir("write-same-mtime");
        let path = dir.join("settings.json");
        fs::write(&path, b"{\"a\":1}").unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let loaded = read_json_file(&path).unwrap();

        // same length, other bytes, and the old modified time put back
        fs::write(&path, b"{\"b\":2}").unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(before).unwrap();
        drop(file);

        let written = write_atomically(&path, b"{\"ours\":1}", loaded.stamp);

        assert!(written.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{\"b\":2}");
        fs::remove_dir_all(&dir).unwrap();
    }
}
