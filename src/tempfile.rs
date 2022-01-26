//! A temporary file in a [`cap_std::fs::Dir`] that may or may not be persisted.
//!
//! At the current time, this API is only implemented on Linux kernels.
//!
//! [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html

use cap_std::fs::{Dir, File, Permissions};
use rustix::fd::{AsFd, FromFd};
use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::path::DecInt;
use std::ffi::OsStr;
use std::io::{Result, Write};
use std::ops::{Deref, DerefMut};
use std::os::unix::prelude::PermissionsExt;
use std::path::Path;

use crate::prelude::CapStdExtDirExt;

fn new_name() -> String {
    #[cfg(not(target_os = "emscripten"))]
    {
        uuid::Uuid::new_v4().to_string()
    }

    // Uuid doesn't support Emscripten yet, but Emscripten isn't multi-user
    // or multi-process yet, so we can do something simple.
    #[cfg(target_os = "emscripten")]
    {
        use rand::RngCore;
        let mut r = rand::thread_rng();
        format!("cap-primitives.{}", r.next_u32())
    }
}

/// A temporary file that may be given a persistent name.
#[derive(Debug)]
pub struct LinkableTempfile<'p, 'd> {
    name: &'p OsStr,
    dir: &'d Dir,
    subdir: Option<Dir>,
    fd: File,
}

impl<'p, 'd> Deref for LinkableTempfile<'p, 'd> {
    type Target = File;

    fn deref(&self) -> &Self::Target {
        &self.fd
    }
}

impl<'p, 'd> std::io::Write for LinkableTempfile<'p, 'd> {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.fd.write(buf)
    }

    fn flush(&mut self) -> Result<()> {
        self.fd.flush()
    }
}

impl<'p, 'd> DerefMut for LinkableTempfile<'p, 'd> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.fd
    }
}

impl<'p, 'd> LinkableTempfile<'p, 'd> {
    pub(crate) fn new_in(dir: &'d Dir, target: &'p Path) -> Result<Self> {
        let name = target.file_name().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Not a file name")
        })?;
        let subdir = if let Some(parent) = target.parent().filter(|v| !v.as_os_str().is_empty()) {
            Some(dir.open_dir(parent)?)
        } else {
            None
        };
        let subdir_fd = subdir.as_ref().unwrap_or(&dir).as_fd();
        // openat's API uses WRONLY.  There may be use cases for reading too, so let's support it.
        let oflags = OFlags::CLOEXEC | OFlags::TMPFILE | OFlags::RDWR;
        let mode = Mode::RUSR | Mode::WUSR;
        let fd = rustix::fs::openat(&subdir_fd, ".", oflags, mode)?;
        let fd = File::from_fd(fd.into());
        Ok(Self {
            name,
            dir,
            subdir,
            fd,
        })
    }

    fn subdir(&self) -> &Dir {
        self.subdir.as_ref().unwrap_or(&self.dir)
    }

    fn try_emplace_to(dir: &Dir, fdname: &DecInt, name: &OsStr) -> rustix::io::Result<()> {
        let procself_fd = rustix::io::proc_self_fd()?;
        rustix::fs::linkat(
            &procself_fd,
            fdname.as_c_str(),
            dir,
            name,
            AtFlags::SYMLINK_FOLLOW,
        )
    }

    /// Write the file to its destination with the chosen permissions.
    pub fn replace_with_perms(self, permissions: Permissions) -> Result<()> {
        let subdir = self.subdir();
        let fd = self.fd.as_fd();
        let procself_fd = rustix::io::proc_self_fd()?;
        let fdnum = rustix::path::DecInt::from_fd(&fd);
        let mut attempts = 0u32;
        let tempname = loop {
            attempts = attempts.saturating_add(1);
            if attempts == u32::MAX {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "too many temporary files exist",
                ));
            }
            let name = new_name();
            match rustix::fs::linkat(
                &procself_fd,
                fdnum.as_c_str(),
                subdir,
                &name,
                AtFlags::SYMLINK_FOLLOW,
            ) {
                Ok(()) => break name,
                Err(e) if e == rustix::io::Error::EXIST => continue,
                Err(e) => return Err(e.into()),
            }
        };
        self.fd.set_permissions(permissions)?;
        // We could avoid the error here if we infallibly constructed the uuid as CString.
        let tempname = rustix::ffi::ZString::new(tempname)?;
        // TODO: panic-safe cleanup of the tempfile.  But I think the only case where we could panic is OOM
        match rustix::fs::renameat(subdir, &tempname, subdir, self.name) {
            Ok(()) => Ok(()),
            Err(e) => {
                // Clean up our temporary file, but ignore errors doing so because
                // the real error was probably from the rename(), and we don't want to mask it.
                let _ = rustix::fs::unlinkat(subdir, tempname, AtFlags::empty());
                Err(e.into())
            }
        }
    }

    /// Write the given contents to the file to its destination with the chosen permissions.
    pub fn replace_contents_using_perms(
        mut self,
        contents: impl AsRef<[u8]>,
        permissions: Permissions,
    ) -> Result<()> {
        self.write_all(contents.as_ref())?;
        self.replace_with_perms(permissions)
    }

    /// Write the given contents to the file to its destination with the chosen permissions.
    pub fn replace_contents(mut self, contents: impl AsRef<[u8]>) -> Result<()> {
        self.write_all(contents.as_ref())?;
        let permissions = self.default_permissions()?;
        self.replace_with_perms(permissions)
    }

    /// Write the file to its destination.
    ///
    /// If a file exists at the destination already, and no override permissions are set, the permissions
    /// will be set to match the destination. Otherwise, a conservative default of `0o600` i.e. `rw-------`
    /// will be used.
    pub fn replace(self) -> Result<()> {
        let permissions = self.default_permissions()?;
        self.replace_with_perms(permissions)
    }

    fn default_permissions(&self) -> Result<Permissions> {
        let permissions = if let Some(p) = self.subdir().metadata_optional(self.name)? {
            p.permissions()
        } else {
            Permissions::from_mode(0o600)
        };
        Ok(permissions)
    }

    /// Write the file to its destination, erroring out if there is an extant file.
    pub fn emplace(self) -> Result<()> {
        let fdnum = rustix::path::DecInt::from_fd(&self.fd);
        Self::try_emplace_to(self.subdir(), &fdnum, self.name).map_err(|e| e.into())
    }
}
