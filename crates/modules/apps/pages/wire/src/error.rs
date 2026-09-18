use sdk::Error;
use sdk::refusal;

/// deterministic module failures. Operation errors abort the whole block (the
/// sdk `abort_block` contract); query errors leave state untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageError {
    /// insert/create of a block id already present ANYWHERE in the module —
    /// block ids are globally unique, that is the addressability contract.
    DuplicateBlock(String),
    ManagedPage,
    RecordUnauthorized,
    RecordCollectionExists(String),
    RecordCollectionNotFound(String),
    InvalidRecordCollection,
    RecordRevisionConflict,
    RecordRequestConflict,
    InvalidRecordBatch,
    RecordNotFound(String),
    RecordStateNotFound(String),
    TooManyRecordStateKeys,
    FilesNotConfigured,
    TooManyRecords,
    /// update/move/remove/check of a block id not in the store.
    BlockNotFound(String),
    /// an insert/move named a parent block that does not exist.
    ParentNotFound(String),
    /// an `after` anchor that is not a child of the named parent.
    AnchorNotFound(String),
    /// a page-block query cursor that is absent or outside the requested page.
    InvalidPageCursor,
    /// a page query exceeded its deterministic block-read budget before the
    /// wasm host's broader per-dispatch store-read ceiling.
    PageTraversalTooDeep,
    /// an insert or move would put a block below [`crate::MAX_PAGE_DEPTH`].
    PageTooDeep,
    /// a subtree-deepening move was too large to validate inside one wasm
    /// dispatch. Same-depth and shallower moves do not need this traversal.
    MoveSubtreeTooLarge,
    /// a page move's physical ancestry exceeded the local store-read budget.
    MoveAncestryTooDeep,
    /// a subtree removal exceeded the local traversal/work budget during preflight.
    RemoveSubtreeTooLarge,
    /// a move whose new parent sits inside the moved block's own subtree.
    CycleMove,
    /// a move whose new parent belongs to a different page.
    CrossPageMove,
    /// `SetKind` tried to convert to or from `Page`. Page membership changes
    /// only through insert/move/remove so the enumeration index stays exact.
    PageKindImmutable,
    /// a non-page block tried to move without a parent.
    TopLevelNonPage,
    /// `SetChecked` on a non-`Todo` block.
    NotTodo,
    /// an inline mark/comment anchor is empty, outside the target text, or
    /// splits a UTF-16 surrogate pair.
    InvalidTextRange,
    /// normalized inline formatting exceeded the per-block span cap.
    TooManySpanMarks,
    /// the op would grow a serialized block (or the index) past
    /// [`MAX_BLOCK_LEN`] — rejected at write time so the oversized bytes never
    /// reach the panicking commit/read paths (the codec bound is decode-only).
    BlockTooLarge,
    /// a `Page` block's text — its title — exceeded [`crate::MAX_PAGE_TITLE_LEN`].
    /// bounded so a full page-list reply stays inside its query byte budget
    /// whatever a client names its pages.
    TitleTooLarge,
    /// stored state failed to decode or a tree invariant is broken (a listed
    /// child missing, a parent chain looping). distinct from absence:
    /// corruption must surface loudly, never masquerade as "not found".
    Corrupt,
    /// an op named the reserved [`PAGE_INDEX_KEY`] sentinel.
    ReservedId,
    /// a block/page or comment op arrived with an empty (pre-consensus)
    /// origin — the actor resolver rejects it before any op can
    /// derive an author from it.
    EmptyOrigin,
    /// an external or module origin was too large for a bounded stored
    /// author (a page's recorded author or a comment's).
    AuthorTooLarge,
    // ── comments ──
    ThreadNotFound(String),
    /// edit/delete named a comment id not in the store (or a tombstone).
    CommentNotFound(String),
    /// AddComment reused a comment id already present.
    DuplicateComment(String),
    /// an append named a target that differs from the thread's; `target` is
    /// the thread's own.
    TargetMismatch {
        thread: String,
        target: String,
    },
    /// comment text over [`MAX_COMMENT_TEXT_BYTES`].
    TextTooLarge,
    /// an AddComment thread_id/comment_id/target over its length cap —
    /// bounded so the derived index/thread blocks can never exceed
    /// [`MAX_BLOCK_LEN`] and abort a block.
    IdTooLarge,
    /// a thread already holds [`MAX_COMMENTS_PER_THREAD`] comments.
    TooManyComments,
    /// a target already holds [`MAX_THREADS_PER_TARGET`] threads.
    TooManyThreads,
    /// the enumeration index already holds [`crate::MAX_PAGES`] pages.
    TooManyPages,
    /// a target's aggregate thread+comment work already sits at
    /// [`MAX_COMMENT_WORK_PER_TARGET`] — the shared removal-work budget for
    /// that target is spent, however many comments any single thread holds.
    TooMuchCommentWork,
}

impl PageError {
    /// the [`refusal`] class a caller branches on; the sentence
    /// ([`Display`](core::fmt::Display)) names the thing refused.
    pub fn class(&self) -> &'static str {
        match self {
            PageError::BlockNotFound(_)
            | PageError::ParentNotFound(_)
            | PageError::AnchorNotFound(_)
            | PageError::ThreadNotFound(_)
            | PageError::CommentNotFound(_)
            | PageError::RecordNotFound(_)
            | PageError::RecordCollectionNotFound(_)
            | PageError::RecordStateNotFound(_) => refusal::NOT_FOUND,
            PageError::DuplicateBlock(_)
            | PageError::DuplicateComment(_)
            | PageError::RecordCollectionExists(_) => refusal::ALREADY_EXISTS,
            // a page cursor names a block; one that is gone or moved off the
            // page is re-read from the start, not fixed.
            PageError::RecordRevisionConflict
            | PageError::RecordRequestConflict
            | PageError::InvalidPageCursor => refusal::STALE,
            PageError::ManagedPage
            | PageError::PageKindImmutable
            | PageError::TopLevelNonPage
            | PageError::NotTodo
            | PageError::CycleMove
            | PageError::CrossPageMove
            | PageError::TargetMismatch { .. }
            | PageError::InvalidRecordCollection
            | PageError::ReservedId
            | PageError::EmptyOrigin
            | PageError::InvalidTextRange
            | PageError::InvalidRecordBatch => refusal::INVALID_INPUT,
            PageError::TooManyRecordStateKeys
            | PageError::TooManyRecords
            | PageError::TooManySpanMarks
            | PageError::TooManyComments
            | PageError::TooManyThreads
            | PageError::TooManyPages
            | PageError::TooMuchCommentWork
            | PageError::BlockTooLarge
            | PageError::TitleTooLarge
            | PageError::TextTooLarge
            | PageError::IdTooLarge
            | PageError::AuthorTooLarge
            | PageError::PageTooDeep
            | PageError::PageTraversalTooDeep
            | PageError::MoveSubtreeTooLarge
            | PageError::MoveAncestryTooDeep
            | PageError::RemoveSubtreeTooLarge => refusal::CAPACITY,
            PageError::RecordUnauthorized => refusal::UNAUTHORIZED,
            PageError::FilesNotConfigured => refusal::UNSUPPORTED,
            PageError::Corrupt => refusal::CORRUPT,
        }
    }
}

impl core::fmt::Display for PageError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            PageError::DuplicateBlock(id) => return write!(f, "Block {id} already exists."),
            PageError::RecordCollectionExists(id) => {
                return write!(f, "Record collection {id} already exists.");
            }
            PageError::RecordCollectionNotFound(id) => {
                return write!(f, "Record collection {id} does not exist.");
            }
            PageError::RecordNotFound(id) => return write!(f, "Record {id} does not exist."),
            PageError::RecordStateNotFound(key) => {
                return write!(f, "Record state key {key} does not exist.");
            }
            PageError::BlockNotFound(id) => return write!(f, "Block {id} does not exist."),
            PageError::ParentNotFound(id) => return write!(f, "Parent block {id} does not exist."),
            PageError::AnchorNotFound(id) => {
                return write!(f, "Block {id} is not a child of the named parent.");
            }
            PageError::ThreadNotFound(id) => return write!(f, "Thread {id} does not exist."),
            PageError::CommentNotFound(id) => return write!(f, "Comment {id} does not exist."),
            PageError::DuplicateComment(id) => return write!(f, "Comment {id} already exists."),
            PageError::TargetMismatch { thread, target } => {
                return write!(f, "Thread {thread} belongs to {target}.");
            }
            PageError::ManagedPage => "A managed page changes only through commit_records.",
            PageError::RecordUnauthorized => "This writer may not write these records.",
            PageError::InvalidRecordCollection => {
                "A record collection needs a top-level page without nested pages."
            }
            PageError::RecordRevisionConflict => {
                "The record collection has moved past the revision this request read."
            }
            PageError::RecordRequestConflict => {
                "This record request id was already used with a different payload."
            }
            PageError::InvalidRecordBatch => "The record batch is invalid or too large.",
            PageError::TooManyRecordStateKeys => "The collection holds too many record state keys.",
            PageError::FilesNotConfigured => "Record artifacts need a configured files module.",
            PageError::TooManyRecords => "The collection holds too many records.",
            PageError::InvalidPageCursor => "The page cursor names no block in this page.",
            PageError::PageTraversalTooDeep => "The page read exceeded its block-read budget.",
            PageError::PageTooDeep => "The block would sit below the page nesting limit.",
            PageError::MoveSubtreeTooLarge => "The subtree is too large to move deeper.",
            PageError::MoveAncestryTooDeep => "The page ancestry is too deep to move.",
            PageError::RemoveSubtreeTooLarge => "The subtree is too large to remove.",
            PageError::CycleMove => "The move target is inside the moved subtree.",
            PageError::CrossPageMove => "A block cannot move to another page.",
            PageError::PageKindImmutable => "A block cannot be converted to or from a page.",
            PageError::TopLevelNonPage => "Only page blocks may move to the top level.",
            PageError::NotTodo => "Only a todo block can be checked.",
            PageError::InvalidTextRange => {
                "The text range is empty, outside the text, or splits a character."
            }
            PageError::TooManySpanMarks => "The block carries too many inline marks.",
            PageError::BlockTooLarge => "The block is too large.",
            PageError::TitleTooLarge => "The page title is too large.",
            PageError::Corrupt => "Stored page state is corrupt.",
            PageError::ReservedId => "The block id is reserved.",
            PageError::EmptyOrigin => "The operation carries an empty origin.",
            PageError::AuthorTooLarge => "The author is too large to record.",
            PageError::TextTooLarge => "The comment text is too large.",
            PageError::IdTooLarge => "The comment id or target is too large.",
            PageError::TooManyComments => "The thread holds too many comments.",
            PageError::TooManyThreads => "The target holds too many threads.",
            PageError::TooManyPages => "The page limit is reached.",
            PageError::TooMuchCommentWork => {
                "The target's comments and threads exceed its removal budget."
            }
        };
        f.write_str(s)
    }
}

/// bridge the only sdk error `load_block` can raise — a stored-block json
/// decode failure — back into `PageError` so `apply` stays single-error-typed.
/// if it ever fires it MUST surface as corruption, not absence: mapping a
/// decode failure to "not found" would let `CreatePage` silently re-seed a
/// root over the corrupt bytes, destroying the evidence AND the data.
pub fn to_page_err(_e: Error) -> PageError {
    PageError::Corrupt
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the class is what a caller branches on; the sentence names the thing.
    #[test]
    fn a_page_error_names_its_class_and_the_thing_refused() {
        let missing = PageError::BlockNotFound("b1".into());
        assert_eq!(missing.class(), refusal::NOT_FOUND);
        assert_eq!(missing.to_string(), "Block b1 does not exist.");

        assert_eq!(PageError::TooManyPages.class(), refusal::CAPACITY);
        assert_eq!(
            PageError::TooManyPages.to_string(),
            "The page limit is reached."
        );
    }
}
