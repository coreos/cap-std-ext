//! Extensions for [`std::process::Command`] that operate on concepts from cap-std.
//!
//! The key APIs here are:
//!
//! - File descriptor passing
//! - Changing to a file-descriptor relative directory

use cap_std::fs::Dir;
use cap_std::io_lifetimes;
use cap_tempfile::cap_std;
use io_lifetimes::OwnedFd;
use rustix::fd::{AsFd, FromRawFd, IntoRawFd};
use rustix::io::FdFlags;
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::sync::Arc;

/// Extension trait for [`std::process::Command`].
///
/// [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html
pub trait CapStdExtCommandExt {
    /// Pass a file descriptor into the target process.
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self;

    /// Use the given directory as the current working directory for the process.
    fn cwd_dir(&mut self, dir: Dir) -> &mut Self;
}

#[allow(unsafe_code)]
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

    fn cwd_dir(&mut self, dir: Dir) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                rustix::process::fchdir(dir.as_fd())?;
                Ok(())
            });
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_take_fdn() -> anyhow::Result<()> {
        // Pass srcfd == destfd and srcfd != destfd
        for i in 0..1 {
            let tempd = cap_tempfile::TempDir::new(cap_std::ambient_authority())?;
            let tempd_fd = Arc::new(tempd.as_fd().try_clone_to_owned()?);
            let n = tempd_fd.as_raw_fd() + i;
            let st = std::process::Command::new("ls")
                .arg(format!("/proc/self/fd/{n}"))
                .take_fd_n(tempd_fd, n)
                .status()?;
            assert!(st.success());
        }
        Ok(())
    }
}
