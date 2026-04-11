//! Lightweight per-profile pidfile for multi-agent exclusivity.
//!
//! Each running `aivyx` server process acquires a pidfile at
//! `<profile-root>/aivyx.pid` on startup. The file is created atomically
//! (O_CREAT | O_EXCL) so two processes racing to start the same profile
//! cannot both succeed. The holding guard removes the file on `Drop`.
//!
//! Stale pidfiles (left behind by a crash) are detected by checking whether
//! the recorded PID is still alive via `kill(pid, 0)` on Unix. A stale file
//! is overwritten; a live one causes acquisition to fail. On non-Unix
//! platforms the stale-detection step is skipped — a pre-existing file is
//! always treated as "held".
//!
//! This module intentionally keeps a tiny surface: `PidFile::acquire` and
//! `PidFile::read_peer`. Anything fancier (process-group locking, named
//! semaphores, D-Bus ownership) is out of scope — this is a best-effort
//! advisory guard, not a distributed lock.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// RAII guard over a profile's pidfile. Removes the file on drop.
#[derive(Debug)]
pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    /// Acquire exclusive ownership of the pidfile at `path`.
    ///
    /// - If the file does not exist, it is created atomically and populated
    ///   with the current PID.
    /// - If the file exists but its PID is no longer alive, the file is
    ///   overwritten (stale pidfile from a crashed process).
    /// - If the file exists and its PID *is* alive, returns `Err` with a
    ///   human-readable message naming the holding PID.
    pub fn acquire(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Check for an existing pidfile and whether the holder is alive.
        if let Ok(existing_pid) = read_pid_from(&path) {
            if pid_is_alive(existing_pid) {
                anyhow::bail!(
                    "pidfile at {} is held by live process {}; \
                     another aivyx process is already running for this profile",
                    path.display(),
                    existing_pid
                );
            }
            // Stale — remove it before retrying the atomic create so our
            // create_new(true) below observes a clean slate.
            let _ = fs::remove_file(&path);
        }

        // Atomic create. If another racing process wins the create, we fail.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to create pidfile at {}: {e} \
                     (another aivyx process may have started concurrently)",
                    path.display()
                )
            })?;

        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;

        Ok(Self { path })
    }

    /// Read the PID recorded in a peer profile's pidfile, returning `None`
    /// if the file is absent, unreadable, malformed, or names a dead process.
    ///
    /// Used by the desktop-exclusivity check to ask "is sibling profile X
    /// currently running?" without touching the file ourselves.
    pub fn read_peer(path: impl AsRef<Path>) -> Option<u32> {
        let pid = read_pid_from(path.as_ref()).ok()?;
        if pid_is_alive(pid) { Some(pid) } else { None }
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        // Best effort: if the cleanup fails (disk full, permissions changed),
        // the next invocation will observe a stale pidfile and recover via
        // the liveness check in `acquire`.
        let _ = fs::remove_file(&self.path);
    }
}

fn read_pid_from(path: &Path) -> std::io::Result<u32> {
    let mut buf = String::new();
    File::open(path)?.read_to_string(&mut buf)?;
    buf.trim()
        .parse::<u32>()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // `kill(pid, sig)` has several special values we must guard against
    // before we cast into `pid_t` (which is signed):
    //   - pid == 0  → signal the caller's process group
    //   - pid == -1 → broadcast to every process the caller can signal
    //   - pid <  -1 → signal the process group with ID -pid
    //
    // None of these mean "check if this specific process is alive". If the
    // stored value is 0 or would cast into a non-positive `pid_t`, it can't
    // name a real process, so we treat it as dead.
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }

    // kill(pid, 0) probes for process existence without actually signalling:
    // - returns 0 if the process exists and we have permission to signal it
    // - sets errno = ESRCH if the process does not exist
    // - sets errno = EPERM if it exists but we lack permission (still alive!)
    //
    // We treat ESRCH as "dead" and everything else as "alive/uncertain" — the
    // safer bias for a startup guard is to assume alive when unsure.
    //
    // `std::io::Error::last_os_error()` is used rather than touching
    // `__errno_location()` / `__error()` directly, because the two platforms
    // expose errno via different symbols and `std` already abstracts that.
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error();
    errno != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    // Without a portable liveness probe, treat any existing pidfile as held.
    // Users on non-Unix platforms running into stale pidfiles can delete the
    // file manually — this is documented in the module-level comment.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nanosecond-precision, per-process-unique suffix for tempdir names.
    ///
    /// Avoids pulling in `rand` as a dev-dependency just for collision-free
    /// test paths. Combining the clock reading with the current PID is
    /// enough for the single-machine test suite; if two threads call this
    /// within the same nanosecond (extremely unlikely in practice) they may
    /// collide, but the tests each create distinct parent directories so
    /// the blast radius is zero.
    fn unique_suffix() -> u128 {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        ts.wrapping_mul(1_000_003) ^ (std::process::id() as u128)
    }

    #[test]
    fn acquire_creates_pidfile_and_drop_removes_it() {
        let dir = std::env::temp_dir().join(format!("aivyx-pidfile-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aivyx.pid");

        {
            let _guard = PidFile::acquire(&path).unwrap();
            assert!(path.exists(), "pidfile should exist while guard is held");

            let recorded = read_pid_from(&path).unwrap();
            assert_eq!(recorded, std::process::id());
        }

        assert!(!path.exists(), "pidfile should be removed when guard drops");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn acquire_refuses_when_live_holder_exists() {
        let dir = std::env::temp_dir().join(format!("aivyx-pidfile-live-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aivyx.pid");

        let _guard = PidFile::acquire(&path).unwrap();
        let err = PidFile::acquire(&path).unwrap_err();
        assert!(
            err.to_string().contains("already running"),
            "expected 'already running' in error, got: {err}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn acquire_recovers_stale_pidfile() {
        let dir = std::env::temp_dir().join(format!("aivyx-pidfile-stale-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aivyx.pid");

        // PID 1 exists on every Unix system (init), so we need a PID that
        // reliably does NOT exist. Use u32::MAX which is well beyond any
        // real kernel pid_max.
        fs::write(&path, format!("{}\n", u32::MAX)).unwrap();

        let guard = PidFile::acquire(&path).unwrap();
        let recorded = read_pid_from(&path).unwrap();
        assert_eq!(
            recorded,
            std::process::id(),
            "stale pidfile should have been overwritten with our PID"
        );
        drop(guard);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_peer_returns_none_for_missing_file() {
        let path =
            std::env::temp_dir().join(format!("aivyx-pidfile-absent-{}.pid", unique_suffix()));
        assert!(PidFile::read_peer(&path).is_none());
    }

    #[test]
    fn read_peer_returns_none_for_dead_pid() {
        let dir = std::env::temp_dir().join(format!("aivyx-pidfile-peerdead-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aivyx.pid");
        fs::write(&path, format!("{}\n", u32::MAX)).unwrap();

        assert!(PidFile::read_peer(&path).is_none());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_peer_returns_our_pid_for_live_holder() {
        let dir = std::env::temp_dir().join(format!("aivyx-pidfile-peerlive-{}", unique_suffix()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("aivyx.pid");

        let _guard = PidFile::acquire(&path).unwrap();
        let peer = PidFile::read_peer(&path).unwrap();
        assert_eq!(peer, std::process::id());

        fs::remove_dir_all(&dir).ok();
    }
}
