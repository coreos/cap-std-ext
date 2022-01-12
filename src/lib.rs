//! # Extension APIs for cap-std
//!
//! This crate builds on top of [`cap-std`].
//!

// See https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![forbid(unused_must_use)]
#![deny(unsafe_code)]
#![cfg_attr(feature = "dox", feature(doc_cfg))]

/// Extension APIs for Command
pub mod cmdext;
