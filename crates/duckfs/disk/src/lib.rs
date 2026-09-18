//! native disk persistence for duckfs.

mod commit;
mod disk;
mod scratch;

pub use commit::{commit_refs, gc_due, persist_objects};
pub use disk::{DiskRefs, DiskStore};
pub use scratch::SyncScratch;

/// the backing store refused a write; an operator looks. A domain class: the
/// request is not at fault and nothing stored is corrupt, so no canonical
/// [`sdk::refusal`] class fits. Reusable by every disk write site.
pub const STORAGE: &str = "storage";
