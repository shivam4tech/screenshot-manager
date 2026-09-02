//! shotmemory-core: the local indexing engine for Screenshot Memory.
//!
//! This crate is deliberately free of any UI-framework dependency so it can be
//! compiled and tested everywhere, and later reused (CLI, service, tests).
//!
//! Modules:
//! - [`error`]: shared error type
//! - [`platform`]: platform-specific default paths (Windows / Linux / macOS)
//! - [`db`]: SQLite persistence layer (schema, migrations, queries)
//! - [`hashing`]: SHA-256 content hashes and dHash perceptual hashes
//! - [`metadata`]: cheap image metadata extraction (dimensions, format)
//! - [`thumbnails`]: thumbnail generation and on-disk cache layout
//! - [`scanner`]: resumable, non-destructive directory scanner
//!
//! Core invariants:
//! - The scanner only ever READS user files. No file mutation happens here.
//! - One problematic file must never stop a scan; errors go to the `problems` table.
//! - Scans are resumable: fingerprinted records make re-scans cheap and safe.

pub mod db;
pub mod error;
pub mod hashing;
pub mod metadata;
pub mod platform;
pub mod scanner;
pub mod thumbnails;

pub use error::{CoreError, CoreResult};
