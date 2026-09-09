use parking_lot::Mutex;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum LockError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Project is locked by another writer (PID: {pid})")]
    AlreadyLocked { pid: u32 },
}

static LOCKED_PROJECTS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Exclusive single-writer lock for an `.aero` project bundle directory.
/// Enforces true mutual exclusion both within the same process and across processes.
pub struct ProjectLock {
    canonical_project_dir: PathBuf,
    lock_path: PathBuf,
    _file: File,
    #[cfg(unix)]
    _directory_lease: File,
}

impl ProjectLock {
    /// Attempts to acquire an exclusive lock file in `project_dir/.lock`.
    /// Fails if another thread in the current process or another OS process holds the lock.
    pub fn acquire<P: AsRef<Path>>(project_dir: P) -> Result<Self, LockError> {
        let p_ref = project_dir.as_ref();
        let canonical_dir = p_ref.canonicalize().unwrap_or_else(|_| p_ref.to_path_buf());
        #[cfg(unix)]
        let directory_lease = File::open(&canonical_dir)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(directory_lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) }
                != 0
            {
                return Err(LockError::AlreadyLocked { pid: 0 });
            }
        }
        let lock_path = canonical_dir.join(".lock");

        // 1. In-process check: ensure no other thread in this process holds the lock
        {
            let mut locked_set = LOCKED_PROJECTS.lock();
            if locked_set.contains(&canonical_dir) {
                return Err(LockError::AlreadyLocked {
                    pid: std::process::id(),
                });
            }
            locked_set.insert(canonical_dir.clone());
        }

        // 2. Cross-process check: open lock file and acquire exclusive OS advisory lock
        let open_res = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path);

        let mut file = match open_res {
            Ok(f) => f,
            Err(e) => {
                let mut locked_set = LOCKED_PROJECTS.lock();
                locked_set.remove(&canonical_dir);
                return Err(LockError::Io(e));
            }
        };

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if ret != 0 {
                // OS lock acquisition failed: another process holds the lock
                let mut content = String::new();
                let _ = file.read_to_string(&mut content);
                let existing_pid = content.trim().parse::<u32>().unwrap_or(0);

                let mut locked_set = LOCKED_PROJECTS.lock();
                locked_set.remove(&canonical_dir);

                return Err(LockError::AlreadyLocked { pid: existing_pid });
            }
        }

        // Lock acquired: write current process PID
        let my_pid = std::process::id();
        let _ = file.set_len(0);
        let _ = file.seek(SeekFrom::Start(0));
        let _ = writeln!(file, "{}", my_pid);
        let _ = file.sync_all();

        Ok(Self {
            canonical_project_dir: canonical_dir,
            lock_path,
            _file: file,
            #[cfg(unix)]
            _directory_lease: directory_lease,
        })
    }
}

impl Drop for ProjectLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(self._file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        let _ = std::fs::remove_file(&self.lock_path);

        let mut locked_set = LOCKED_PROJECTS.lock();
        locked_set.remove(&self.canonical_project_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_lock_acquisition_and_release() {
        let dir = tempdir().unwrap();
        let lock1 = ProjectLock::acquire(dir.path()).unwrap();
        assert!(dir.path().join(".lock").exists());

        // Concurrent acquisition in the same process must be rejected
        let lock2_res = ProjectLock::acquire(dir.path());
        assert!(matches!(lock2_res, Err(LockError::AlreadyLocked { .. })));

        // Release first lock
        drop(lock1);
        assert!(!dir.path().join(".lock").exists());

        // Now can acquire again cleanly
        let lock3 = ProjectLock::acquire(dir.path()).unwrap();
        assert!(dir.path().join(".lock").exists());
        drop(lock3);
    }
}
