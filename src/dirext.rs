//! Extensions for [`cap_std::fs::Dir`].
//!
//! [`cap_std::fs::Dir`]: https://docs.rs/cap-std/latest/cap_std/fs/struct.Dir.html

use cap_std::fs::{Dir, File, Metadata};
use std::io::Result;
use std::path::Path;

/// Extension trait for [`cap_std::fs::Dir`]
pub trait CapStdExtDirExt {
    /// Open a file read-only, but return `Ok(None)` if it does not exist.
    fn open_optional(&self, path: impl AsRef<Path>) -> Result<Option<File>>;

    /// Open a directory, but return `Ok(None)` if it does not exist.
    fn open_dir_optional(&self, path: impl AsRef<Path>) -> Result<Option<Dir>>;

    /// Gather metadata, but return `Ok(None)` if it does not exist.
    fn metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>>;

    /// Remove (delete) a file, but return `Ok(false)` if the file does not exist.
    fn remove_file_optional(&self, path: impl AsRef<Path>) -> Result<bool>;

    /// Create a new anonymous file that can be given a persistent name.
    /// On Linux, this uses `O_TMPFILE` if possible, otherwise a randomly named
    /// temporary file is used.  
    ///
    /// The file can later be linked into place once it has been completely written.
    #[cfg(any(target_os = "android", target_os = "linux"))]
    fn new_linkable_file<'p, 'd>(
        &'d self,
        path: &'p Path,
    ) -> Result<crate::tempfile::LinkableTempfile<'p, 'd>>;
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

impl CapStdExtDirExt for Dir {
    fn open_optional(&self, path: impl AsRef<Path>) -> Result<Option<File>> {
        map_optional(self.open(path.as_ref()))
    }

    fn open_dir_optional(&self, path: impl AsRef<Path>) -> Result<Option<Dir>> {
        map_optional(self.open_dir(path.as_ref()))
    }

    fn metadata_optional(&self, path: impl AsRef<Path>) -> Result<Option<Metadata>> {
        map_optional(self.metadata(path.as_ref()))
    }

    fn remove_file_optional(&self, path: impl AsRef<Path>) -> Result<bool> {
        match self.remove_file(path.as_ref()) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    #[cfg(any(target_os = "android", target_os = "linux"))]
    fn new_linkable_file<'p, 'd>(
        &'d self,
        target: &'p Path,
    ) -> Result<crate::tempfile::LinkableTempfile<'p, 'd>> {
        crate::tempfile::LinkableTempfile::new_in(self, target)
    }
}
