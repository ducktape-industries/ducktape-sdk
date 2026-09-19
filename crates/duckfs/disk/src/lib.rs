//! native disk persistence for duckfs.

mod commit;
mod disk;
mod scratch;

pub use commit::{commit_refs, gc_due, persist_objects};
pub use disk::{DiskRefs, DiskStore};
pub use scratch::SyncScratch;

// the `storage` domain class lives with `FsRefusal`, whose `Storage` variant
// carries it: one constant, one home. Re-exported so a disk write site names it
// without reaching past this crate.
pub use duckfs_core::STORAGE;
