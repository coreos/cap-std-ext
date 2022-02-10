//! A temporary file in a [`cap_std::fs::Dir`] that may or may not be persisted.
//!
//! At the current time, this API is only implemented on Linux kernels.
//!
//! [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html

use cap_std::fs::{Dir, File, Permissions};
use rustix::fd::{AsFd, FromFd};
use rustix::fs::{AtFlags, Mode, OFlags, OpenOptionsExt};
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

/// Create a new temporary file in the target directory, which may or may not have a (randomly generated) name at this point.
fn new_tempfile(d: &Dir) -> Result<(File, Option<rustix::ffi::ZString>)> {
    // openat's API uses WRONLY.  There may be use cases for reading too, so let's support it.
    let oflags = OFlags::CLOEXEC | OFlags::TMPFILE | OFlags::RDWR;
    let mode = Mode::RUSR | Mode::WUSR;
    // Happy path - Linux with O_TMPFILE
    match rustix::fs::openat(d, ".", oflags, mode) {
        Ok(r) => return Ok(((File::from_fd(r.into())), None)),
        Err(e) if e == rustix::io::Error::OPNOTSUPP => {}
        Err(e) => {
            return Err(e.into());
        }
    };
    // Otherwise, fall back
    let mut attempts = 0u32;
    let mut opts = cap_std::fs::OpenOptions::new();
    opts.read(true);
    opts.write(true);
    opts.create_new(true);
    opts.mode(mode.as_raw_mode());
    loop {
        attempts = attempts.saturating_add(1);
        if attempts == u32::MAX {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "too many temporary files exist",
            ));
        }
        let name = new_name();
        match d.open_with(&name, &opts) {
            Ok(r) => return Ok((r, Some(rustix::ffi::ZString::new(name)?))),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Assign a random name to a currently anonymous O_TMPFILE descriptor.
fn generate_name_in(subdir: &Dir, f: &File) -> Result<rustix::ffi::ZString> {
    let procself_fd = rustix::io::proc_self_fd()?;
    let fdnum = rustix::path::DecInt::from_fd(&f.as_fd());
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
    Ok(rustix::ffi::ZString::new(tempname)?)
}

/// A temporary file that may be given a persistent name.
#[derive(Debug)]
pub struct LinkableTempfile<'p, 'd> {
    name: &'p OsStr,
    dir: &'d Dir,
    subdir: Option<Dir>,
    fd: File,
    tempname: Option<rustix::ffi::ZString>,
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
        let (fd, tempname) = new_tempfile(subdir.as_ref().unwrap_or(dir))?;
        Ok(Self {
            name,
            dir,
            subdir,
            fd,
            tempname,
        })
    }

    fn subdir(&self) -> &Dir {
        self.subdir.as_ref().unwrap_or(self.dir)
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
    pub fn replace_with_perms(mut self, permissions: Permissions) -> Result<()> {
        let subdir = self.subdir.take();
        let subdir = subdir.as_ref().unwrap_or(self.dir);
        self.fd.set_permissions(permissions)?;
        // Take ownership of the temporary name now
        let tempname = if let Some(t) = self.tempname.take() {
            t
        } else {
            generate_name_in(subdir, &self.fd)?
        };
        // And try the rename.
        rustix::fs::renameat(subdir, &tempname, subdir, self.name).map_err(|e| {
            // But, if we catch an error here, then move ownership back into self,
            // whioch means the Drop invocation will clean it up.
            self.tempname = Some(tempname);
            e.into()
        })
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
        let fdnum = rustix::path::DecInt::from_fd(&self.fd.as_fd());
        Self::try_emplace_to(self.subdir(), &fdnum, self.name).map_err(|e| e.into())
    }
}

impl<'p, 'd> Drop for LinkableTempfile<'p, 'd> {
    fn drop(&mut self) {
        if let Some(name) = self.tempname.take() {
            let _ = rustix::fs::unlinkat(self.subdir(), name, AtFlags::empty());
        }
    }
}
