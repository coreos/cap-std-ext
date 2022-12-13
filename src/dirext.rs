//! Extensions for [`cap_std::fs::Dir`].
//!
//! [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html

use cap_std::fs::{Dir, File, Metadata};
use cap_tempfile::cap_std;
use std::ffi::OsStr;
use std::io::Result;
use std::io::{self, Write};
use std::ops::Deref;
use std::path::Path;

/// Extension trait for [`cap_std::fs::Dir`]
pub trait CapStdExtDirExt {
    /// Open a file read-only, but return `Ok(None)` if it does not exist.
    fn open_optional(&self, path: impl AsRef<Path>) -> Result<Option<File>>;

    /// Open a directory, but return `Ok(None)` if it does not exist.
    fn open_dir_optional(&self, path: impl AsRef<Path>) -> Result<Option<Dir>>;

    /// Create the target directory, but do nothing if a directory already exists at that path.
    /// The return value will be `true` if the directory was created.  An error will be
    /// returned if the path is a non-directory.  Symbolic links will be followed.
    fn ensure_dir_with(
        &self,
        p: impl AsRef<Path>,
        builder: &cap_std::fs::DirBuilder,
    ) -> Result<bool>;

    /// Gather metadata, but return `Ok(None)` if it does not exist.
    fn metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>>;

    /// Gather metadata (but do not follow symlinks), but return `Ok(None)` if it does not exist.
    fn symlink_metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>>;

    /// Remove (delete) a file, but return `Ok(false)` if the file does not exist.
    fn remove_file_optional(&self, path: impl AsRef<Path>) -> Result<bool>;

    /// Remove a file or directory but return `Ok(false)` if the file does not exist.
    /// Symbolic links are not followed.
    fn remove_all_optional(&self, path: impl AsRef<Path>) -> Result<bool>;

    /// Set the access and modification times to the current time.  Symbolic links are not followed.
    #[cfg(unix)]
    fn update_timestamps(&self, path: impl AsRef<Path>) -> Result<()>;

    /// Atomically write a file by calling the provided closure.
    ///
    /// This uses [`cap_tempfile::TempFile`], which is wrapped in a [`std::io::BufWriter`]
    /// and passed to the closure.
    ///
    /// The closure may also perform other file operations beyond writing, such as changing
    /// file permissions:
    ///
    /// ```rust
    /// # use std::io;
    /// # use std::io::Write;
    /// # use cap_tempfile::cap_std;
    /// # fn main() -> io::Result<()> {
    /// # let somedir = cap_tempfile::tempdir(cap_std::ambient_authority())?;
    /// use cap_std_ext::prelude::*;
    /// let contents = b"hello world\n";
    /// somedir.atomic_replace_with("somefilename", |f| -> io::Result<_> {
    ///     f.write_all(contents)?;
    ///     f.flush()?;
    ///     use std::os::unix::prelude::PermissionsExt;
    ///     let perms = cap_std::fs::Permissions::from_mode(0o600);
    ///     f.get_mut().as_file_mut().set_permissions(perms)?;
    ///     Ok(())
    /// })
    /// # }
    /// ```
    ///
    /// Any existing file will be replaced.
    fn atomic_replace_with<F, T, E>(
        &self,
        destname: impl AsRef<Path>,
        f: F,
    ) -> std::result::Result<T, E>
    where
        F: FnOnce(&mut std::io::BufWriter<cap_tempfile::TempFile>) -> std::result::Result<T, E>,
        E: From<std::io::Error>;

    /// Atomically write the provided contents to a file.
    fn atomic_write(&self, destname: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()>;

    /// Atomically write the provided contents to a file, using specified permissions.
    fn atomic_write_with_perms(
        &self,
        destname: impl AsRef<Path>,
        contents: impl AsRef<[u8]>,
        perms: cap_std::fs::Permissions,
    ) -> Result<()>;
}

fn map_optional<R>(r: Result<R>) -> Result<Option<R>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }
}

enum DirOwnedOrBorrowed<'d> {
    Owned(Dir),
    Borrowed(&'d Dir),
}

impl<'d> Deref for DirOwnedOrBorrowed<'d> {
    type Target = Dir;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(d) => d,
            Self::Borrowed(d) => d,
        }
    }
}

/// Given a directory reference and a path, if the path includes a subdirectory (e.g. on Unix has a `/`)
/// then open up the target directory, and return the file name.
///
/// Otherwise, reborrow the directory and return the file name.
///
/// It is an error if the target path does not name a file.
fn subdir_of<'d, 'p>(d: &'d Dir, p: &'p Path) -> io::Result<(DirOwnedOrBorrowed<'d>, &'p OsStr)> {
    let name = p
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Not a file name"))?;
    let r = if let Some(subdir) = p
        .parent()
        .filter(|v| !v.as_os_str().is_empty())
        .map(|p| d.open_dir(p))
    {
        DirOwnedOrBorrowed::Owned(subdir?)
    } else {
        DirOwnedOrBorrowed::Borrowed(d)
    };
    Ok((r, name))
}

impl CapStdExtDirExt for Dir {
    fn open_optional(&self, path: impl AsRef<Path>) -> Result<Option<File>> {
        map_optional(self.open(path.as_ref()))
    }

    fn open_dir_optional(&self, path: impl AsRef<Path>) -> Result<Option<Dir>> {
        map_optional(self.open_dir(path.as_ref()))
    }

    fn ensure_dir_with(
        &self,
        p: impl AsRef<Path>,
        builder: &cap_std::fs::DirBuilder,
    ) -> Result<bool> {
        let p = p.as_ref();
        match self.create_dir_with(p, builder) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if !self.symlink_metadata(p)?.is_dir() {
                    // TODO use https://doc.rust-lang.org/std/io/enum.ErrorKind.html#variant.NotADirectory
                    // once it's stable.
                    return Err(io::Error::new(io::ErrorKind::Other, "Found non-directory"));
                }
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    fn metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>> {
        map_optional(self.metadata(path.as_ref()))
    }

    fn symlink_metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>> {
        map_optional(self.symlink_metadata(path.as_ref()))
    }

    fn remove_file_optional(&self, path: impl AsRef<Path>) -> Result<bool> {
        match self.remove_file(path.as_ref()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn remove_all_optional(&self, path: impl AsRef<Path>) -> Result<bool> {
        let path = path.as_ref();
        // This is obviously racy, but correctly matching on the errors
        // runs into the fact that e.g. https://doc.rust-lang.org/std/io/enum.ErrorKind.html#variant.NotADirectory
        // is unstable right now.
        let meta = match self.symlink_metadata_optional(path)? {
            Some(m) => m,
            None => return Ok(false),
        };
        if meta.is_dir() {
            self.remove_dir_all(path)?;
        } else {
            self.remove_file(path)?;
        }
        Ok(true)
    }

    #[cfg(unix)]
    fn update_timestamps(&self, path: impl AsRef<Path>) -> Result<()> {
        use rustix::fd::AsFd;
        use rustix::fs::UTIME_NOW;

        let path = path.as_ref();
        let now = rustix::fs::Timespec {
            tv_sec: 0,
            tv_nsec: UTIME_NOW,
        };
        // https://github.com/bytecodealliance/rustix/commit/69af396b79e296717bece8148b1f6165b810885c
        // means that Timespec only implements Copy on 64 bit right now.
        #[allow(clippy::clone_on_copy)]
        let times = rustix::fs::Timestamps {
            last_access: now.clone(),
            last_modification: now.clone(),
        };
        rustix::fs::utimensat(
            self.as_fd(),
            path,
            &times,
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )?;
        Ok(())
    }

    fn atomic_replace_with<F, T, E>(
        &self,
        destname: impl AsRef<Path>,
        f: F,
    ) -> std::result::Result<T, E>
    where
        F: FnOnce(&mut std::io::BufWriter<cap_tempfile::TempFile>) -> std::result::Result<T, E>,
        E: From<std::io::Error>,
    {
        let destname = destname.as_ref();
        let (d, name) = subdir_of(self, destname)?;
        let t = cap_tempfile::TempFile::new(&d)?;
        let mut bufw = std::io::BufWriter::new(t);
        let r = f(&mut bufw)?;
        bufw.into_inner()
            .map_err(From::from)
            .and_then(|t| t.replace(name))?;
        Ok(r)
    }

    fn atomic_write(&self, destname: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Result<()> {
        self.atomic_replace_with(destname, |f| f.write_all(contents.as_ref()))
    }

    fn atomic_write_with_perms(
        &self,
        destname: impl AsRef<Path>,
        contents: impl AsRef<[u8]>,
        perms: cap_std::fs::Permissions,
    ) -> Result<()> {
        self.atomic_replace_with(destname, |f| -> io::Result<_> {
            // If the user is overriding the permissions, let's make the default be
            // writable by us but not readable by anyone else, in case it has
            // secret data.
            #[cfg(unix)]
            {
                use std::os::unix::prelude::PermissionsExt;
                let perms = cap_std::fs::Permissions::from_mode(0o600);
                f.get_mut().as_file_mut().set_permissions(perms)?;
            }
            f.write_all(contents.as_ref())?;
            f.flush()?;
            f.get_mut().as_file_mut().set_permissions(perms)?;
            Ok(())
        })
    }
}
