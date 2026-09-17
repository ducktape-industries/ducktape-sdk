//! the one place the plain git types and their WIT-generated twins meet.
//!
//! [`git_primitives`] is what a substrate returns and what a module's read
//! policy names; [`crate::bindings`] is what the guest lifts. They are the same
//! shapes declared twice, because `bindgen!`'s `with:` cannot map a record
//! (see [`crate::GitObject`]), so the correspondence is spelled out here
//! instead of asserted by the macro. A field added to `wit/module.wit` without
//! being added to `git-primitives` fails to compile in this file — which is the
//! point of keeping the conversions in one place rather than at the call sites.

use crate::bindings::ducktape::module::host as wit;

/// an object crosses whole: the host neither parses it nor re-shapes it, so
/// this is a field rename and nothing else. What a commit or a tree MEANS is
/// decided guest-side (`git_primitives::parse_commit` / `parse_tree`).
pub(crate) fn object(object: git_primitives::GitObject) -> wit::GitObject {
    wit::GitObject {
        kind: object.kind,
        size: object.size,
        raw: object.raw,
    }
}

/// both halves at once: a diff read answers with its own error type, so the
/// memo holds a plain `Result` and the guest gets the WIT one.
pub(crate) fn diff_result(
    answer: Result<git_primitives::GitDiff, git_primitives::GitDiffError>,
) -> Result<wit::GitDiff, wit::GitDiffError> {
    answer.map(diff).map_err(diff_error)
}

fn diff(diff: git_primitives::GitDiff) -> wit::GitDiff {
    wit::GitDiff {
        patch: diff.patch,
        truncated: diff.truncated,
        files_changed: diff.files_changed,
        additions: diff.additions,
        deletions: diff.deletions,
        files: diff.files.into_iter().map(diff_file).collect(),
    }
}

fn diff_file(file: git_primitives::GitDiffFile) -> wit::GitDiffFile {
    wit::GitDiffFile {
        path: file.path,
        previous_path: file.previous_path,
        status: match file.status {
            git_primitives::GitFileStatus::Added => wit::GitFileStatus::Added,
            git_primitives::GitFileStatus::Modified => wit::GitFileStatus::Modified,
            git_primitives::GitFileStatus::Deleted => wit::GitFileStatus::Deleted,
            git_primitives::GitFileStatus::Renamed => wit::GitFileStatus::Renamed,
            git_primitives::GitFileStatus::TypeChanged => wit::GitFileStatus::TypeChanged,
        },
        additions: file.additions,
        deletions: file.deletions,
        binary: file.binary,
        truncated: file.truncated,
    }
}

fn diff_error(error: git_primitives::GitDiffError) -> wit::GitDiffError {
    match error {
        git_primitives::GitDiffError::Unavailable(message) => wit::GitDiffError::Unavailable(message),
        git_primitives::GitDiffError::Limit(message) => wit::GitDiffError::Limit(message),
        git_primitives::GitDiffError::Unsupported => wit::GitDiffError::Unsupported,
    }
}
