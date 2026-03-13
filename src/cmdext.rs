//! Extensions for [`std::process::Command`] that operate on concepts from cap-std.
//!
//! The key APIs here are:
//!
//! - File descriptor passing
//! - Changing to a file-descriptor relative directory
//! - Systemd socket activation fd passing

use cap_std::fs::Dir;
use cap_std::io_lifetimes;
use cap_tempfile::cap_std;
use io_lifetimes::OwnedFd;
use rustix::fd::{AsFd, FromRawFd, IntoRawFd};
use rustix::io::FdFlags;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::sync::Arc;

/// The file descriptor number at which systemd passes the first socket.
/// See `sd_listen_fds(3)`.
const SD_LISTEN_FDS_START: i32 = 3;

/// A validated name for a systemd socket-activation file descriptor.
///
/// Names appear in the `LISTEN_FDNAMES` environment variable as
/// colon-separated values, so they must not contain `:`.
/// The constructor validates this at construction time.
///
/// ```
/// use cap_std_ext::cmdext::SystemdFdName;
/// let name = SystemdFdName::new("varlink");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SystemdFdName<'a>(&'a str);

impl<'a> SystemdFdName<'a> {
    /// Create a new `SystemdFdName`, panicking if `name` contains `:`.
    pub fn new(name: &'a str) -> Self {
        assert!(!name.contains(':'), "systemd fd name must not contain ':'");
        Self(name)
    }

    /// Return the name as a string slice.
    pub fn as_str(&self) -> &'a str {
        self.0
    }
}

/// File descriptor allocator for child processes.
///
/// Collects fd assignments and optional systemd socket-activation
/// configuration, then applies them all at once via
/// [`CapStdExtCommandExt::take_fds`].
///
/// - [`new_systemd_fds`](Self::new_systemd_fds) creates an allocator
///   with systemd socket-activation fds at 3, 4, … (`SD_LISTEN_FDS_START`).
/// - [`take_fd`](Self::take_fd) auto-assigns the next fd above all
///   previously assigned ones (minimum 3).
/// - [`take_fd_n`](Self::take_fd_n) places an fd at an explicit number,
///   panicking on overlap.
///
/// ```no_run
/// # use std::sync::Arc;
/// # use cap_std_ext::cmdext::{CmdFds, CapStdExtCommandExt, SystemdFdName};
/// # let varlink_fd: Arc<rustix::fd::OwnedFd> = todo!();
/// # let extra_fd: Arc<rustix::fd::OwnedFd> = todo!();
/// let mut cmd = std::process::Command::new("myservice");
/// let mut fds = CmdFds::new_systemd_fds([(varlink_fd, SystemdFdName::new("varlink"))]);
/// let extra_n = fds.take_fd(extra_fd);
/// cmd.take_fds(fds);
/// ```
#[derive(Debug)]
pub struct CmdFds {
    taken: BTreeSet<i32>,
    fds: Vec<(i32, Arc<OwnedFd>)>,
    /// Pre-built CStrings for the systemd env vars, set by new_systemd_fds.
    systemd_env: Option<(CString, CString)>,
}

impl Default for CmdFds {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdFds {
    /// Create a new fd allocator.
    pub fn new() -> Self {
        Self {
            taken: BTreeSet::new(),
            fds: Vec::new(),
            systemd_env: None,
        }
    }

    /// Create a new fd allocator with systemd socket-activation fds.
    ///
    /// Each `(fd, name)` pair is assigned a consecutive fd number starting
    /// at `SD_LISTEN_FDS_START` (3). The `LISTEN_PID`, `LISTEN_FDS`, and
    /// `LISTEN_FDNAMES` environment variables will be set in the child
    /// when [`CapStdExtCommandExt::take_fds`] is called.
    ///
    /// Additional (non-systemd) fds can be registered afterwards via
    /// [`take_fd`](Self::take_fd) or [`take_fd_n`](Self::take_fd_n).
    ///
    /// [sd_listen_fds]: https://www.freedesktop.org/software/systemd/man/latest/sd_listen_fds.html
    pub fn new_systemd_fds<'a>(
        fds: impl IntoIterator<Item = (Arc<OwnedFd>, SystemdFdName<'a>)>,
    ) -> Self {
        let mut this = Self::new();
        this.register_systemd_fds(fds);
        this
    }

    /// Compute the next fd number above everything already taken
    /// (minimum `SD_LISTEN_FDS_START`).
    fn next_fd(&self) -> i32 {
        self.taken
            .last()
            .map(|n| n.checked_add(1).expect("fd number overflow"))
            .unwrap_or(SD_LISTEN_FDS_START)
    }

    fn insert_fd(&mut self, n: i32) {
        let inserted = self.taken.insert(n);
        assert!(inserted, "fd {n} is already assigned");
    }

    /// Register a file descriptor at the next available fd number.
    ///
    /// Returns the fd number that will be assigned in the child.
    /// Call [`CapStdExtCommandExt::take_fds`] to apply.
    pub fn take_fd(&mut self, fd: Arc<OwnedFd>) -> i32 {
        let n = self.next_fd();
        self.insert_fd(n);
        self.fds.push((n, fd));
        n
    }

    /// Register a file descriptor at a specific fd number.
    ///
    /// Call [`CapStdExtCommandExt::take_fds`] to apply.
    ///
    /// # Panics
    ///
    /// Panics if `target` has already been assigned.
    pub fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self {
        self.insert_fd(target);
        self.fds.push((target, fd));
        self
    }

    fn register_systemd_fds<'a>(
        &mut self,
        fds: impl IntoIterator<Item = (Arc<OwnedFd>, SystemdFdName<'a>)>,
    ) {
        let mut n_fds: i32 = 0;
        let mut names = Vec::new();
        for (fd, name) in fds {
            let target = SD_LISTEN_FDS_START
                .checked_add(n_fds)
                .expect("too many fds");
            self.insert_fd(target);
            self.fds.push((target, fd));
            names.push(name.as_str());
            n_fds = n_fds.checked_add(1).expect("too many fds");
        }

        let fd_count = CString::new(n_fds.to_string()).unwrap();
        // SAFETY: SystemdFdName guarantees no NUL bytes.
        let fd_names = CString::new(names.join(":")).unwrap();
        self.systemd_env = Some((fd_count, fd_names));
    }
}

/// Extension trait for [`std::process::Command`].
///
/// [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html
pub trait CapStdExtCommandExt {
    /// Pass a file descriptor into the target process at a specific fd number.
    #[deprecated = "Use CmdFds with take_fds() instead"]
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self;

    /// Apply a [`CmdFds`] to this command, passing all registered file
    /// descriptors and (if configured) setting up the systemd
    /// socket-activation environment.
    fn take_fds(&mut self, fds: CmdFds) -> &mut Self;

    /// Use the given directory as the current working directory for the process.
    fn cwd_dir(&mut self, dir: Dir) -> &mut Self;

    /// On Linux, arrange for [`SIGTERM`] to be delivered to the child if the
    /// parent *thread* exits. This helps avoid leaking child processes if
    /// the parent crashes for example.
    ///
    /// # IMPORTANT
    ///
    /// Due to the semantics of <https://man7.org/linux/man-pages/man2/prctl.2.html> this
    /// will cause the child to exit when the parent *thread* (not process) exits. In
    /// particular this can become problematic when used with e.g. a threadpool such
    /// as Tokio's <https://kobzol.github.io/rust/2025/02/23/tokio-plus-prctl-equals-nasty-bug.html>.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn lifecycle_bind_to_parent_thread(&mut self) -> &mut Self;
}

/// Wrapper around `libc::setenv` that checks the return value.
///
/// # Safety
///
/// Must only be called in a single-threaded context (e.g. after `fork()`
/// and before `exec()`).
#[allow(unsafe_code)]
unsafe fn check_setenv(key: *const i8, val: *const i8) -> std::io::Result<()> {
    // SAFETY: Caller guarantees we are in a single-threaded context
    // with valid nul-terminated C strings.
    if unsafe { libc::setenv(key, val, 1) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[allow(unsafe_code)]
#[allow(deprecated)]
impl CapStdExtCommandExt for std::process::Command {
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                let mut target = OwnedFd::from_raw_fd(target);
                // If the fd is already what we want, then just ensure that
                // O_CLOEXEC is stripped off.
                if target.as_raw_fd() == fd.as_raw_fd() {
                    let fl = rustix::io::fcntl_getfd(&target)?;
                    rustix::io::fcntl_setfd(&mut target, fl.difference(FdFlags::CLOEXEC))?;
                } else {
                    // Otherwise create a dup, which will also default to not setting O_CLOEXEC.
                    rustix::io::dup2(&*fd, &mut target)?;
                }
                // Intentionally leak into the child.
                let _ = target.into_raw_fd();
                Ok(())
            });
        }
        self
    }

    fn take_fds(&mut self, fds: CmdFds) -> &mut Self {
        for (target, fd) in fds.fds {
            self.take_fd_n(fd, target);
        }
        if let Some((fd_count, fd_names)) = fds.systemd_env {
            // Set LISTEN_PID/FDS/FDNAMES in the forked child via setenv(3).
            // We cannot use Command::env() because it causes Rust to build
            // an envp array that replaces environ after our pre_exec setenv
            // calls.
            unsafe {
                self.pre_exec(move || {
                    let pid = rustix::process::getpid();
                    let pid_dec = rustix::path::DecInt::new(pid.as_raw_nonzero().get());
                    // SAFETY: After fork() and before exec(), the child is
                    // single-threaded, so setenv (which is not thread-safe)
                    // is safe to call here.
                    check_setenv(c"LISTEN_PID".as_ptr(), pid_dec.as_c_str().as_ptr())?;
                    check_setenv(c"LISTEN_FDS".as_ptr(), fd_count.as_ptr())?;
                    check_setenv(c"LISTEN_FDNAMES".as_ptr(), fd_names.as_ptr())?;
                    Ok(())
                });
            }
        }
        self
    }

    fn cwd_dir(&mut self, dir: Dir) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                rustix::process::fchdir(dir.as_fd())?;
                Ok(())
            });
        }
        self
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn lifecycle_bind_to_parent_thread(&mut self) -> &mut Self {
        // SAFETY: This API is safe to call in a forked child.
        unsafe {
            self.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::TERM,
                ))
                .map_err(Into::into)
            });
        }
        self
    }
}

#[cfg(all(test, any(target_os = "android", target_os = "linux")))]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[allow(deprecated)]
    #[test]
    fn test_take_fdn() -> anyhow::Result<()> {
        // Pass srcfd == destfd and srcfd != destfd
        for i in 0..=1 {
            let tempd = cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
            let tempd_fd = Arc::new(tempd.as_fd().try_clone_to_owned()?);
            let n = tempd_fd.as_raw_fd() + i;
            #[cfg(any(target_os = "android", target_os = "linux"))]
            let path = format!("/proc/self/fd/{n}");
            #[cfg(not(any(target_os = "android", target_os = "linux")))]
            let path = format!("/dev/fd/{n}");
            let st = std::process::Command::new("/usr/bin/env")
                .arg("readlink")
                .arg(path)
                .take_fd_n(tempd_fd, n)
                .status()?;
            assert!(st.success());
        }
        Ok(())
    }
}
