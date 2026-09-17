//! the bounded local-git read surface, as plain Rust data.
//!
//! the `ducktape:module/host` git types, hand-written here with no dependencies
//! so that BOTH sides of a git read can name them: the kernel host declares
//! them across [`wasm_host::OdbBacking`] and converts them to its own
//! WIT-generated twins inside the import implementations, while a module's read
//! policy names them directly and so stays repo-separable from the kernel
//! (#2303). the field-for-field correspondence with `wit/module.wit` is the
//! contract; `crates/kernel/wasm-host/src/git_wit.rs` is the one place the two
//! shapes meet, so a WIT edit that does not reach here fails to compile there.
//!
//! an OBJECT read is the exception, and deliberately so: the host hands over
//! the raw object and nothing else, and [`parse_commit`] / [`parse_tree`] turn
//! those bytes into [`GitCommit`] / [`GitTreeEntry`] GUEST-side. neither of
//! those two is a WIT shape any more, so what a reader wants to know about a
//! commit grows here — in a plain Rust crate — instead of in the world every
//! component is compiled against.
//!
//! [`wasm_host::OdbBacking`]: ../wasm_host/trait.OdbBacking.html

/// git's own object type numbering, as [`GitObject::kind`] carries it.
pub const KIND_COMMIT: u8 = 1;
pub const KIND_TREE: u8 = 2;
pub const KIND_BLOB: u8 = 3;
pub const KIND_TAG: u8 = 4;

/// one object as the substrate stores it: its kind, its true size, and its
/// body in git's own format — the bytes `git cat-file <type> <oid>` prints,
/// with no header and no compression.
///
/// `raw` is EMPTY when the read's `max_bytes` refused to materialize the body,
/// so the body is whole exactly when `raw.len() as u64 == size`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitObject {
    pub kind: u8,
    pub size: u64,
    pub raw: Vec<u8>,
}

/// a commit as a history walk needs it. `author` is the raw identity line
/// ("Name <email>"), `committed_at` the COMMITTER's epoch seconds — the order
/// a branch was built in, which is the order a log is read in. `message` is
/// the whole message; the first line is a summary only by convention, and the
/// parser does not get to decide that for its caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCommit {
    pub tree: Vec<u8>,
    pub parents: Vec<Vec<u8>>,
    pub author: String,
    pub committed_at: u64,
    pub message: String,
}

/// one entry of a tree: the high nibble of its octal mode (4 a directory,
/// 8 a regular file, 10 a symlink, 14 a submodule), the path component as
/// stored, and the target oid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitTreeEntry {
    pub kind: u8,
    pub name: Vec<u8>,
    pub oid: Vec<u8>,
}

/// why an object body was not the object it was read as. One reason per
/// malformation, spelled for a log line rather than matched on: a caller's only
/// honest response to any of them is to refuse the read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitParseError(pub &'static str);

impl core::fmt::Display for GitParseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for GitParseError {}

/// Parse a commit object body ([`GitObject::raw`] of a [`KIND_COMMIT`] read).
///
/// Headers run to the first empty line and the rest is the message. Unknown
/// headers and their folded continuation lines (`gpgsig`'s armor) are skipped,
/// not refused: a signed commit is an ordinary commit to a history walk.
pub fn parse_commit(raw: &[u8]) -> Result<GitCommit, GitParseError> {
    let (mut tree, mut author, mut committed_at) = (None, None, None);
    let mut parents = Vec::new();
    let mut rest = raw;
    while !rest.is_empty() {
        let (line, tail) =
            split_once(rest, b'\n').ok_or(GitParseError("a commit header is unterminated"))?;
        rest = tail;
        match line.first() {
            // the empty line that ends the headers: everything left is message.
            None => break,
            // a folded continuation of the header above it.
            Some(b' ') => continue,
            _ => {}
        }
        let (name, value) =
            split_once(line, b' ').ok_or(GitParseError("a commit header has no value"))?;
        match name {
            b"tree" => tree = Some(oid(value)?),
            b"parent" => parents.push(oid(value)?),
            b"author" => author = Some(identity(value)?),
            b"committer" => committed_at = Some(epoch_seconds(value)?),
            _ => {}
        }
    }
    Ok(GitCommit {
        tree: tree.ok_or(GitParseError("a commit has no tree"))?,
        parents,
        author: author.ok_or(GitParseError("a commit has no author"))?,
        committed_at: committed_at.ok_or(GitParseError("a commit has no committer"))?,
        message: String::from_utf8_lossy(rest).into_owned(),
    })
}

/// Parse a tree object body ([`GitObject::raw`] of a [`KIND_TREE`] read):
/// `<octal mode> <name>\0<20-byte oid>`, repeated, in git's own stored order.
pub fn parse_tree(raw: &[u8]) -> Result<Vec<GitTreeEntry>, GitParseError> {
    let mut entries = Vec::new();
    let mut rest = raw;
    while !rest.is_empty() {
        let (head, tail) =
            split_once(rest, 0).ok_or(GitParseError("a tree entry has no name terminator"))?;
        let (mode, name) =
            split_once(head, b' ').ok_or(GitParseError("a tree entry has no mode"))?;
        if name.is_empty() {
            return Err(GitParseError("a tree entry has no name"));
        }
        let (id, tail) = tail
            .split_at_checked(20)
            .ok_or(GitParseError("a tree entry has no oid"))?;
        entries.push(GitTreeEntry {
            kind: mode_nibble(mode)?,
            name: name.to_vec(),
            oid: id.to_vec(),
        });
        rest = tail;
    }
    Ok(entries)
}

/// split on the FIRST `sep`, dropping it; `None` when it does not occur.
fn split_once(bytes: &[u8], sep: u8) -> Option<(&[u8], &[u8])> {
    let at = bytes.iter().position(|byte| *byte == sep)?;
    Some((&bytes[..at], &bytes[at + 1..]))
}

/// 40 hex characters as git writes an oid, into the 20 bytes it stores.
fn oid(hex: &[u8]) -> Result<Vec<u8>, GitParseError> {
    if hex.len() != 40 {
        return Err(GitParseError("an oid is 40 hex characters"));
    }
    hex.chunks(2)
        .map(|pair| Ok(nibble(pair[0])? << 4 | nibble(pair[1])?))
        .collect()
}

fn nibble(digit: u8) -> Result<u8, GitParseError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        b'A'..=b'F' => Ok(digit - b'A' + 10),
        _ => Err(GitParseError("an oid is hexadecimal")),
    }
}

/// `Name <email> <epoch> <tz>` → the identity line alone. The timezone is the
/// committer's own display preference, not a fact about the commit, and epoch
/// seconds are already absolute — so neither belongs in a name.
fn identity(value: &[u8]) -> Result<String, GitParseError> {
    let end = value
        .iter()
        .rposition(|byte| *byte == b'>')
        .ok_or(GitParseError("an identity line ends with <email>"))?;
    Ok(String::from_utf8_lossy(&value[..=end]).into_owned())
}

/// the epoch seconds of a `Name <email> <epoch> <tz>` line.
fn epoch_seconds(value: &[u8]) -> Result<u64, GitParseError> {
    let end = value
        .iter()
        .rposition(|byte| *byte == b'>')
        .ok_or(GitParseError("an identity line ends with <email>"))?;
    let seconds = value[end + 1..]
        .split(|byte| *byte == b' ')
        .find(|field| !field.is_empty())
        .ok_or(GitParseError("an identity line has no timestamp"))?;
    core::str::from_utf8(seconds)
        .ok()
        .and_then(|seconds| seconds.parse().ok())
        .ok_or(GitParseError("a timestamp is epoch seconds"))
}

/// the high nibble of a tree entry's octal mode — the only part of it that
/// says what the entry IS. git writes it without a leading zero (`40000`).
fn mode_nibble(mode: &[u8]) -> Result<u8, GitParseError> {
    if mode.is_empty() || mode.len() > 6 {
        return Err(GitParseError("a tree entry mode is 1 to 6 octal digits"));
    }
    let mut bits: u32 = 0;
    for digit in mode {
        if !matches!(digit, b'0'..=b'7') {
            return Err(GitParseError("a tree entry mode is octal"));
        }
        bits = bits << 3 | u32::from(digit - b'0');
    }
    Ok((bits >> 12) as u8)
}

/// the ceilings one diff read may spend, carried together because they are one
/// policy decision rather than three: how much patch text, how many files, and
/// how many blob bytes this reply is allowed to materialize.
///
/// `max_files` is ignored by a path-scoped read, which examines exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitDiffBudget {
    pub max_bytes: u64,
    pub max_files: u64,
    pub max_blob_bytes: u64,
}

/// what happened to one path between two commits. a closed set: a new kind
/// must fail the build wherever it is matched rather than land in a wildcard
/// that renders it as "modified".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    TypeChanged,
}

/// one path's row in a diff's index. the index is COMPLETE even when the patch
/// is not, so a reader can always see which files changed and navigate them.
///
/// `additions`/`deletions` are `None`, not zero, when this file's patch was
/// never produced — the byte ceiling stopped the walk before it, or the file's
/// own blobs were too large to examine. a zero meaning "unknown" is a lie the
/// reader cannot detect; `None` cannot be misread. `truncated` says why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDiffFile {
    pub path: String,
    /// the previous path, set only when `status` is [`GitFileStatus::Renamed`].
    /// Spelled out rather than `from`, which the WIT this mirrors cannot use.
    pub previous_path: Option<String>,
    pub status: GitFileStatus,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
    pub truncated: bool,
}

/// a diff between two commits, with the counts that stay true even when
/// `patch` was clipped at the ceiling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDiff {
    pub patch: String,
    pub truncated: bool,
    pub files_changed: u64,
    pub additions: u64,
    pub deletions: u64,
    /// every changed path, ordered by path. `files_changed` is its length.
    pub files: Vec<GitDiffFile>,
}

/// why a diff could not be answered. `Unsupported` is the substrate saying it
/// has no git plane at all, which is a different thing from a read that hit a
/// ceiling ([`GitDiffError::Limit`]) or a repository that would not open
/// ([`GitDiffError::Unavailable`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDiffError {
    Unavailable(String),
    Limit(String),
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `git cat-file commit 62c1690`, verbatim: the smallest real commit in
    /// this repository's own history.
    const COMMIT: &[u8] =
        b"tree 20f623c2e2d97a6e8ae455cacf0106431b1e6763\nparent 33b09079314fc699aeca8042cb93db4d\
        894c5728\nauthor Eddy <hong@byeongsu.dev> 1782952058 +0900\ncommitter GitHub <noreply@\
        github.com> 1782952058 +0900\n\nsetup shared workflow skills";
    /// the first four lines of `git cat-file commit 22b10d8`'s `gpgsig`, the
    /// only header in this history whose value folds over further lines.
    const ARMOR: &[u8] =
        b"gpgsig -----BEGIN PGP SIGNATURE-----\n \n wsFcBAABCAAQBQJqq83nCRC1aQ7uu5UhlAAAcAwQADd6\
        e2wI9jPm/c8iOM4+KvSt\n /4NyUbF8lg/aFtqAC7YDiL4mJhZENI7X5EcNJk8lL6CqJ7PDp+UAmMPejLQOYum\
        o\n";
    /// `git cat-file tree HEAD:crates/git-primitives`, verbatim: a regular
    /// file and a subdirectory, which is both mode nibbles a reader sees.
    const TREE: &[u8] =
        b"100644 Cargo.toml\x00\xea`9\xdf\x1d\x06\xbe\xed}IQ\xbbX7\x1a\xa3\xbdl;r40000 src\x00\x16\
        \x1b3\xd3v\xed\x9f\xb6\xe0\x03\xfa\xbb\xa7\x9a\xdb\xe1\xaaxm\x9a";

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn a_real_commit_parses_into_the_fields_a_walk_reads() {
        let commit = parse_commit(COMMIT).expect("a real commit parses");
        assert_eq!(
            hex(&commit.tree),
            "20f623c2e2d97a6e8ae455cacf0106431b1e6763"
        );
        assert_eq!(
            commit.parents.iter().map(|p| hex(p)).collect::<Vec<_>>(),
            ["33b09079314fc699aeca8042cb93db4d894c5728"]
        );
        assert_eq!(commit.author, "Eddy <hong@byeongsu.dev>");
        assert_eq!(commit.committed_at, 1782952058);
        assert_eq!(commit.message, "setup shared workflow skills");
    }

    /// a signature's armor folds over lines that begin with a space. Each of
    /// them is a continuation of `gpgsig`, NOT a header — `wsFcBAAB...` is not
    /// a field name, and a parser that reads it as one loses the commit.
    #[test]
    fn a_folded_header_does_not_become_a_header_of_its_own() {
        let mut signed = Vec::new();
        let split = COMMIT
            .windows(2)
            .position(|pair| pair == b"\n\n")
            .expect("the header/message boundary");
        signed.extend_from_slice(&COMMIT[..=split]);
        signed.extend_from_slice(ARMOR);
        signed.extend_from_slice(&COMMIT[split + 1..]);
        assert_eq!(
            parse_commit(&signed).unwrap(),
            parse_commit(COMMIT).unwrap()
        );
    }

    #[test]
    fn a_real_tree_parses_into_its_entries_in_stored_order() {
        let entries = parse_tree(TREE).expect("a real tree parses");
        let seen: Vec<_> = entries
            .iter()
            .map(|entry| {
                (
                    entry.kind,
                    String::from_utf8_lossy(&entry.name).into_owned(),
                    hex(&entry.oid),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (
                    8,
                    "Cargo.toml".to_string(),
                    "ea6039df1d06beed7d4951bb58371aa3bd6c3b72".to_string()
                ),
                (
                    4,
                    "src".to_string(),
                    "161b33d376ed9fb6e003fabba79adbe1aa786d9a".to_string()
                ),
            ]
        );
    }

    /// every malformation refuses; none of them parses into a plausible lie.
    #[test]
    fn a_malformed_object_is_refused_rather_than_guessed() {
        for body in [
            &b"parent 33b09079314fc699aeca8042cb93db4d894c5728\n\n"[..],
            b"tree 20f623c2\n\n",
            b"tree 20f623c2e2d97a6e8ae455cacf0106431b1e6763\n\n",
            b"tree 20f623c2e2d97a6e8ae455cacf0106431b1e6763\nauthor Eddy <e> 1 +0000\n\n",
            b"tree 20f623c2e2d97a6e8ae455cacf0106431b1e6763",
        ] {
            assert!(
                parse_commit(body).is_err(),
                "{:?}",
                String::from_utf8_lossy(body)
            );
        }
        for body in [
            &b"100644 Cargo.toml\x00short"[..],
            b"100644\x00\xea`9\xdf\x1d\x06\xbe\xed}IQ\xbbX7\x1a\xa3\xbdl;r",
            b"100999 f\x00\xea`9\xdf\x1d\x06\xbe\xed}IQ\xbbX7\x1a\xa3\xbdl;r",
            b"100644 Cargo.toml",
        ] {
            assert!(
                parse_tree(body).is_err(),
                "{:?}",
                String::from_utf8_lossy(body)
            );
        }
    }
}
