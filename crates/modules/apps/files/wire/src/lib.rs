//! the files module's wire surface: duckfs-core, whole. the module adds no
//! wire of its own — its messages, queries, replies and object types are the
//! versioned filesystem's — so this crate is the name the daemon, the CLI and
//! a sibling module link, and every type is `duckfs_core`'s. The one thing it
//! adds is files' half of a `duck://` address, [`FileAddress`], which is not a
//! wire message.

mod address;
pub use address::*;
pub use duckfs_core::*;
