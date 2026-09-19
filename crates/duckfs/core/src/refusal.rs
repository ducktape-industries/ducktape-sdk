//! what a commit or a query refuses with, classed by how the caller recovers.
//!
//! [`Fs::commit`](crate::fs::Fs::commit) and [`Fs::query`](crate::fs::Fs::query)
//! answer with [`FsRefusal`], not a bare sentence. The variant is produced where
//! the failure is DETECTED, so the class is a fact about the site rather than a
//! guess made later by reading the sentence: a caller that sniffed
//! `"conflict:"` off a string is a caller that breaks when the wording changes.
//!
//! An adapter in front of this crate frames a refusal with the sdk's
//! `refusal::encode(e.class(), &e.to_string())`: [`FsRefusal::class`] is the
//! token, [`Display`] is the sentence. Two variants share a class exactly when
//! the caller recovers from them the same way, which is why `BaseUnresolvable`,
//! `ChunkUnavailable`, `PathNotFound` and `SnapshotUnresolvable` are all
//! `not_found` — each is answered by naming a thing that exists — while the
//! sentence says which thing is missing.

use core::fmt;

use refusal_class::{
    ALREADY_EXISTS, CAPACITY, CORRUPT, EXHAUSTED, INVALID_INPUT, NOT_FOUND, STALE, UNAUTHORIZED,
};

/// the backing store refused a read or a write; an operator looks. A domain
/// class: the request is not at fault and nothing stored is corrupt, so no
/// canonical [`refusal_class`] class fits. Reusable by every store site, which
/// is why it lives here and `duckfs-disk` re-exports it.
pub const STORAGE: &str = "storage";

/// a commit or a query refused, and why in a way a caller can branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FsRefusal {
    /// a path in the commit moved between the base snapshot and the head: the
    /// caller re-reads the head and retries.
    CommitConflict { path: String },
    /// the base snapshot the commit names is not in the history window.
    BaseUnresolvable { base: String },
    /// a chunk the commit references is neither committed nor staged.
    ChunkUnavailable { chunk: String },
    /// nothing is stored at the path.
    PathNotFound { path: String },
    /// the path is a file or a symlink where a directory was required.
    NotADirectory { path: String },
    /// the path is a directory or a symlink where a file was required.
    NotAFile { path: String },
    /// something is already stored at the path the operation would create.
    AlreadyExists { path: String },
    /// the snapshot the query names is not in the history window.
    SnapshotUnresolvable { snapshot: String },
    /// the path breaks a static path rule, so no state can make it valid.
    InvalidPath { path: String, why: String },
    /// the request breaks a static rule of the operation itself.
    InvalidRequest { why: String },
    /// the authority may not touch this path.
    Unauthorized { why: String },
    /// a count, size or work bound of the request or the store is hit.
    Capacity { what: String },
    /// the source revision counter cannot advance again; permanent.
    RevisionExhausted,
    /// the object or refs store refused; an operator looks.
    Storage(String),
    /// a stored object or index failed to decode or broke an invariant.
    Corrupt(String),
}

impl FsRefusal {
    /// the refusal class: how the caller recovers.
    pub fn class(&self) -> &'static str {
        match self {
            Self::CommitConflict { .. } => STALE,
            Self::BaseUnresolvable { .. }
            | Self::ChunkUnavailable { .. }
            | Self::PathNotFound { .. }
            | Self::SnapshotUnresolvable { .. } => NOT_FOUND,
            Self::NotADirectory { .. }
            | Self::NotAFile { .. }
            | Self::InvalidPath { .. }
            | Self::InvalidRequest { .. } => INVALID_INPUT,
            Self::AlreadyExists { .. } => ALREADY_EXISTS,
            Self::Unauthorized { .. } => UNAUTHORIZED,
            Self::Capacity { .. } => CAPACITY,
            Self::RevisionExhausted => EXHAUSTED,
            Self::Storage(_) => STORAGE,
            Self::Corrupt(_) => CORRUPT,
        }
    }
}

impl fmt::Display for FsRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // the `conflict:` shape is kept: the frame splits a refusal on its
            // FIRST `": "`, so a sentence may carry a colon of its own.
            Self::CommitConflict { path } => write!(f, "conflict: {path} changed since base"),
            Self::BaseUnresolvable { base } => write!(
                f,
                "base snapshot {base} is not resolvable; it may have been collected out of the history window"
            ),
            Self::ChunkUnavailable { chunk } => {
                write!(f, "chunk {chunk} is not available; upload it again")
            }
            Self::PathNotFound { path } => write!(f, "{path} does not exist"),
            Self::NotADirectory { path } => write!(f, "{path} is not a directory"),
            Self::NotAFile { path } => write!(f, "{path} is not a file"),
            Self::AlreadyExists { path } => write!(f, "{path} already exists"),
            Self::SnapshotUnresolvable { snapshot } => {
                write!(f, "snapshot {snapshot} is not resolvable")
            }
            Self::InvalidPath { path, why } => write!(f, "{path} is not a valid path: {why}"),
            // these three already arrive as whole sentences from the site that
            // detected them; wrapping them would say the subject twice.
            Self::InvalidRequest { why } | Self::Unauthorized { why } => f.write_str(why),
            Self::Capacity { what } => f.write_str(what),
            Self::RevisionExhausted => f.write_str("source revision exhausted"),
            Self::Storage(what) | Self::Corrupt(what) => f.write_str(what),
        }
    }
}

impl std::error::Error for FsRefusal {}

#[cfg(test)]
mod tests {
    use super::*;

    /// the whole point of the type: every variant answers with its class, and a
    /// class is only shared where the recovery is.
    #[test]
    fn every_variant_carries_its_class() {
        let p = || "/home/m/a".to_string();
        let cases = [
            (FsRefusal::CommitConflict { path: p() }, STALE),
            (FsRefusal::BaseUnresolvable { base: "ab".into() }, NOT_FOUND),
            (
                FsRefusal::ChunkUnavailable { chunk: "cd".into() },
                NOT_FOUND,
            ),
            (FsRefusal::PathNotFound { path: p() }, NOT_FOUND),
            (
                FsRefusal::SnapshotUnresolvable {
                    snapshot: "ef".into(),
                },
                NOT_FOUND,
            ),
            (FsRefusal::NotADirectory { path: p() }, INVALID_INPUT),
            (FsRefusal::NotAFile { path: p() }, INVALID_INPUT),
            (
                FsRefusal::InvalidPath {
                    path: p(),
                    why: "path is not NFC-normalized".into(),
                },
                INVALID_INPUT,
            ),
            (
                FsRefusal::InvalidRequest {
                    why: "commit must carry at least one change".into(),
                },
                INVALID_INPUT,
            ),
            (FsRefusal::AlreadyExists { path: p() }, ALREADY_EXISTS),
            (
                FsRefusal::Unauthorized {
                    why: "home root is not writable".into(),
                },
                UNAUTHORIZED,
            ),
            (
                FsRefusal::Capacity {
                    what: "object-read budget exceeded (64)".into(),
                },
                CAPACITY,
            ),
            (FsRefusal::RevisionExhausted, EXHAUSTED),
            (FsRefusal::Storage("write failed".into()), STORAGE),
            (
                FsRefusal::Corrupt("tree entry is not a tree".into()),
                CORRUPT,
            ),
        ];
        for (refusal, class) in cases {
            assert_eq!(refusal.class(), class, "{refusal:?}");
            assert!(!refusal.to_string().is_empty(), "{refusal:?}");
        }
    }

    /// the parity proof between the native and the wasm build keys on this
    /// sentence, so the budget's own wording must survive the wrapping.
    #[test]
    fn a_capacity_sentence_is_passed_through_verbatim() {
        let what = "object-read budget exceeded (64)";
        assert_eq!(
            FsRefusal::Capacity { what: what.into() }.to_string(),
            what,
            "the read-budget needle must reach the caller unchanged"
        );
    }
}
