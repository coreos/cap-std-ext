// See https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html
#![deny(missing_docs)]
#![doc = include_str!("../README.md")]
#![deny(missing_debug_implementations)]
#![forbid(unused_must_use)]
#![deny(unsafe_code)]
#![cfg_attr(feature = "dox", feature(doc_cfg))]

// Re-export our dependencies
pub use cap_std;

#[cfg(not(windows))]
pub mod cmdext;
pub mod dirext;

/// Prelude, intended for glob import.
pub mod prelude {
    pub use super::cmdext::CapStdExtCommandExt;
    pub use super::dirext::CapStdExtDirExt;
}
