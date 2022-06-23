//! Extensions for [`std::process::Command`].

use cap_std::fs::Dir;
use rustix::fd::{AsFd, FromRawFd, IntoRawFd};
use rustix::io::OwnedFd;
use std::ops::Deref;
use std::os::unix::process::CommandExt;
use std::sync::Arc;

/// Extension trait for [`std::process::Command`].
pub trait CapStdExtCommandExt {
    /// Pass a file descriptor into the target process.
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self;

    /// Use the given directory as the current working directory for the process.
    fn cwd_dir<T>(&mut self, dir: Arc<T>) -> &mut Self
    where
        T: Deref<Target = Dir> + Send + Sync + 'static;

    /// Use the given directory as the current working directory for the process.
    /// This command replaces [`cwd_dir`] which due to a mistake in API design
    /// effectively only supports [`cap_tempfile::TempDir`] and not plain [`cap_std::fs::Dir`]
    /// instances.
    fn cwd_dir_owned(&mut self, dir: Dir) -> &mut Self;
}

#[allow(unsafe_code)]
impl CapStdExtCommandExt for std::process::Command {
    fn take_fd_n(&mut self, fd: Arc<OwnedFd>, target: i32) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                let mut target = rustix::io::OwnedFd::from_raw_fd(target);
                rustix::io::dup2(&*fd, &mut target)?;
                // Intentionally leak into the child.
                let _ = target.into_raw_fd();
                Ok(())
            });
        }
        self
    }

    fn cwd_dir<T>(&mut self, dir: Arc<T>) -> &mut Self
    where
        T: Deref<Target = Dir> + Send + Sync + 'static,
    {
        unsafe {
            self.pre_exec(move || {
                rustix::process::fchdir(dir.as_fd())?;
                Ok(())
            });
        }
        self
    }

    fn cwd_dir_owned(&mut self, dir: Dir) -> &mut Self {
        unsafe {
            self.pre_exec(move || {
                rustix::process::fchdir(dir.as_fd())?;
                Ok(())
            });
        }
        self
    }
}
