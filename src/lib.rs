// See https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
#![deny(missing_debug_implementations)]
#![forbid(unused_must_use)]
#![deny(unsafe_code)]
#![cfg_attr(feature = "dox", feature(doc_cfg))]

// Re-export our dependencies
pub use cap_primitives;
#[cfg(feature = "fs_utf8")]
pub use cap_std::fs_utf8::camino;
pub use cap_tempfile;
pub use cap_tempfile::cap_std;

#[cfg(not(windows))]
pub mod cmdext;
pub mod dirext;

#[cfg(any(target_os = "android", target_os = "linux"))]
mod rootdir;
pub use rootdir::*;

/// Prelude, intended for glob import.
pub mod prelude {
    #[cfg(not(windows))]
    pub use super::cmdext::CapStdExtCommandExt;
    pub use super::dirext::CapStdExtDirExt;
    #[cfg(feature = "fs_utf8")]
    pub use super::dirext::CapStdExtDirExtUtf8;
}
