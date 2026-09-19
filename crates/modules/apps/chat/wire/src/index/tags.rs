//! hashtag tags on the derived chat view — extraction, postings, catalog,
//! and the tag queries. node-local like everything in this index: no key
//! written here is ever part of any `root()`/root-hash.
//!
//! key spaces (inside chat's per-module index database, next to `tok/`):
//! - `tag/{label}/{channel}/{rseq}` — one posting per (tag, message), value =
//!   [`TokRef`] like a `tok/` posting. `rseq = u64::MAX - seq` in fixed hex,
//!   so key order within a channel is NEWEST FIRST: a channel-scoped tag page
//!   streams straight off one scan, and a label's newest live seq is the
//!   first posting under its prefix.
//! - `tagcat/{encoded-channel}/{label}` / `tagcat/g/{label}` — [`TagCat`]
//!   live count, mirrored by the count-ranked `tagrank/` marker.
//!
//! extraction grammar (see the design doc): `#` + 1..=64 chars of Unicode
//! letters/digits/`_`/`-`, opened only at start-of-text or
//! after whitespace/punctuation — never mid-word (`foo#bar`), after another
//! `#`, or after `/`/`&` (URL fragments, HTML entities). Paragraph and Quote
//! spans only; Code blocks and Link-marked spans never carry tags. the index
//! label is the NFC-normalized lowercase form; the as-typed display form
//! stays in the message text. at most [`MAX_TAGS_PER_MESSAGE`] distinct
//! labels index per message.

use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use index_guest::{Fail, StateRead, Writes};

use super::{
    DEFAULT_SEARCH_LIMIT, FAIL_BAD_REQUEST, FAIL_ROW_DECODE, MAX_SEARCH_LIMIT, MsgRow, TokRef,
    msg_key,
};
use crate::{Block, Mark};

/// distinct tag labels indexed per message; later tags are dropped.
pub const MAX_TAGS_PER_MESSAGE: usize = 16;
/// chars (not bytes) a tag may carry after the `#`; longer runs are not tags.
pub const MAX_TAG_CHARS: usize = 64;

/// one catalog row of the `Tags` reply: a label, how many live messages carry
/// it (in the channel scope asked), and the newest such message's seq.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagRow {
    pub tag: String,
    pub count: u64,
    pub last_seq: u64,
}

/// the stored catalog value — live-message count only. `last_seq` is read from
/// the newest bounded posting when a catalog row is served.
#[derive(Debug, Serialize, Deserialize)]
struct TagCat {
    count: u64,
}

// ── keys ────────────────────────────────────────────────────────────────────

/// a tag posting's key. `u64::MAX - seq` keeps per-channel postings newest
/// first in key order (seq is per-channel, so the inversion never collides).
#[cfg(test)]
pub(super) fn tag_key(label: &str, channel: &str, seq: u64) -> String {
    tag_key_at(label, channel, seq, seq)
}

pub(super) fn tag_key_at(label: &str, channel: &str, seq: u64, time: u64) -> String {
    format!(
        "tag/{label}/{:016x}/{}/{:016x}",
        u64::MAX - time,
        hex_lower(channel.as_bytes()),
        u64::MAX - seq
    )
}

pub(super) fn tag_channel_key_at(label: &str, channel: &str, seq: u64, time: u64) -> String {
    format!(
        "tagc/{}/{label}/{:016x}/{:016x}",
        hex_lower(channel.as_bytes()),
        u64::MAX - time,
        u64::MAX - seq
    )
}

fn tag_channel_prefix(label: &str, channel: &str) -> String {
    format!("tagc/{}/{label}/", hex_lower(channel.as_bytes()))
}

fn tag_prefix(label: &str) -> String {
    format!("tag/{label}/")
}

pub(super) fn catalog_key(channel: &str, label: &str) -> String {
    format!("tagcat/{}/{label}", hex_lower(channel.as_bytes()))
}

fn global_catalog_key(label: &str) -> String {
    format!("tagcat/g/{label}")
}

fn rank_key(scope: &str, label: &str, count: u64) -> String {
    format!("{scope}{:016x}/{label}", u64::MAX - count)
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// the catalog value for `count` — one encoder so every write path produces
/// byte-identical entries.
fn encode_catalog(count: u64) -> Result<Vec<u8>, Fail> {
    serde_json::to_vec(&TagCat { count }).map_err(|e| Fail::new(FAIL_ROW_DECODE, e.to_string()))
}

// ── extraction ──────────────────────────────────────────────────────────────

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// whether a `#` after `prev` opens a tag: start-of-text or any whitespace /
/// punctuation boundary — but never mid-word, after another `#` (no `##tag`),
/// or after `/` / `&` (URL fragments like `…/#frag`, entities like `&#39;`).
fn opens_tag(prev: Option<char>) -> bool {
    match prev {
        None => true,
        Some(p) => !p.is_alphanumeric() && !matches!(p, '#' | '/' | '&'),
    }
}

/// NFC-normalized lowercase index label of a raw tag body.
pub(super) fn normalize(raw: &str) -> String {
    raw.nfc().collect::<String>().to_lowercase()
}

/// append the labels found in one plain-text run to `out`, deduplicated
/// against what is already there, in appearance order.
fn collect_labels(text: &str, out: &mut Vec<String>) {
    let mut prev: Option<char> = None;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '#' && opens_tag(prev) {
            let rest = &text[i + 1..];
            let mut char_count = 0usize;
            let mut byte_len = 0usize;
            for rc in rest.chars() {
                if !is_tag_char(rc) {
                    break;
                }
                char_count += 1;
                byte_len += rc.len_utf8();
            }
            // consume the whole run either way, so a rejected (over-long) run
            // is never re-entered mid-way as a fresh boundary.
            for _ in 0..char_count {
                chars.next();
            }
            prev = Some(rest[..byte_len].chars().next_back().unwrap_or(c));
            if (1..=MAX_TAG_CHARS).contains(&char_count) {
                let label = normalize(&rest[..byte_len]);
                if !out.contains(&label) {
                    out.push(label);
                }
            }
            continue;
        }
        prev = Some(c);
    }
}

/// the distinct tag labels of one message body, appearance order, capped at
/// [`MAX_TAGS_PER_MESSAGE`]. Paragraph/Quote spans only; Code blocks and
/// Link-marked spans are never scanned.
pub(super) fn labels(blocks: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    for block in blocks {
        let spans = match block {
            Block::Paragraph(spans) | Block::Quote(spans) => spans,
            Block::Code { .. } | Block::Divider => continue,
        };
        for span in spans {
            if span.marks.iter().any(|m| matches!(m, Mark::Link(_))) {
                continue;
            }
            collect_labels(&span.text, &mut out);
        }
    }
    out.truncate(MAX_TAGS_PER_MESSAGE);
    out
}

// ── fold maintenance ────────────────────────────────────────────────────────

/// delete the tag postings of `row`'s current tag set (the edit/delete
/// counterpart of the postings `put_row_and_toks` emits).
pub(super) fn delete_postings(out: &mut Writes, row: &MsgRow) {
    for label in &row.tags {
        index_guest::delete(out, tag_key_at(label, &row.channel_id, row.seq, row.time));
        index_guest::delete(
            out,
            tag_channel_key_at(label, &row.channel_id, row.seq, row.time),
        );
    }
}

/// fold one head transition's tag-set diff into the catalog: labels in `new`
/// but not `old` count up, labels in `old` but not `new` count down (their
/// entry is deleted at zero). a post passes `old = []`, a delete `new = []`.
pub(super) fn fold_catalog(
    read: &impl StateRead,
    out: &mut Writes,
    channel: &str,
    old: &[String],
    new: &[String],
) -> Result<(), Fail> {
    for label in new.iter().filter(|l| !old.contains(l)) {
        bump(read, out, channel, label, 1)?;
    }
    for label in old.iter().filter(|l| !new.contains(l)) {
        bump(read, out, channel, label, -1)?;
    }
    Ok(())
}

fn bump(
    read: &impl StateRead,
    out: &mut Writes,
    channel: &str,
    label: &str,
    delta: i64,
) -> Result<(), Fail> {
    bump_scope(
        read,
        out,
        CatalogScope {
            key: catalog_key(channel, label),
            rank_prefix: format!("tagrank/c/{}/", hex_lower(channel.as_bytes())),
            label,
            delta,
        },
    )?;
    bump_scope(
        read,
        out,
        CatalogScope {
            key: global_catalog_key(label),
            rank_prefix: "tagrank/g/".into(),
            label,
            delta,
        },
    )?;
    Ok(())
}

struct CatalogScope<'a> {
    key: String,
    rank_prefix: String,
    label: &'a str,
    delta: i64,
}

fn bump_scope(
    read: &impl StateRead,
    out: &mut Writes,
    scope: CatalogScope<'_>,
) -> Result<(), Fail> {
    let old = read
        .get(scope.key.as_bytes())
        .map(|bytes| {
            serde_json::from_slice::<TagCat>(&bytes)
                .map_err(|e| Fail::new(FAIL_ROW_DECODE, e.to_string()))
        })
        .transpose()?
        .unwrap_or(TagCat { count: 0 });
    let count = if scope.delta >= 0 {
        old.count + scope.delta as u64
    } else {
        old.count.saturating_sub(scope.delta.unsigned_abs())
    };
    if old.count > 0 {
        index_guest::delete(out, rank_key(&scope.rank_prefix, scope.label, old.count));
    }
    if count == 0 {
        index_guest::delete(out, scope.key);
        return Ok(());
    }
    let cat = encode_catalog(count)?;
    index_guest::put(out, scope.key, cat.clone());
    index_guest::put(out, rank_key(&scope.rank_prefix, scope.label, count), cat);
    Ok(())
}

// ── serving ─────────────────────────────────────────────────────────────────

fn decode_tok(value: &[u8]) -> Result<TokRef, Fail> {
    serde_json::from_slice(value).map_err(|e| Fail::new(FAIL_ROW_DECODE, e.to_string()))
}

fn fixed_hex(value: &str) -> bool {
    value.len() == 16
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn validate_rank_cursor(after: Option<&str>, prefix: &str) -> Result<(), Fail> {
    let Some(cursor) = after else {
        return Ok(());
    };
    let Some((count, label)) = cursor
        .strip_prefix(prefix)
        .and_then(|rest| rest.split_once('/'))
    else {
        return Err(Fail::new(FAIL_BAD_REQUEST, "invalid tag cursor"));
    };
    if !fixed_hex(count)
        || label.is_empty()
        || label.chars().count() > MAX_TAG_CHARS
        || !label.chars().all(is_tag_char)
        || normalize(label) != label
    {
        return Err(Fail::new(FAIL_BAD_REQUEST, "invalid tag cursor"));
    }
    Ok(())
}

fn validate_posting_cursor(
    after: Option<&str>,
    prefix: &str,
    channel_scoped: bool,
) -> Result<(), Fail> {
    let Some(cursor) = after else {
        return Ok(());
    };
    let Some(rest) = cursor.strip_prefix(prefix) else {
        return Err(Fail::new(FAIL_BAD_REQUEST, "invalid tag cursor"));
    };
    let parts: Vec<_> = rest.split('/').collect();
    let valid = match parts.as_slice() {
        [time, seq] if channel_scoped => fixed_hex(time) && fixed_hex(seq),
        [time, channel, seq] if !channel_scoped => {
            fixed_hex(time)
                && channel.len() % 2 == 0
                && channel
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
                && fixed_hex(seq)
        }
        _ => false,
    };
    if !valid {
        return Err(Fail::new(FAIL_BAD_REQUEST, "invalid tag cursor"));
    }
    Ok(())
}

/// the `Tags` query: the catalog of one channel (or, with no channel, every
/// channel aggregated per label), ordered count desc then tag asc, clamped
/// like search.
pub(super) fn serve_tags(
    read: &impl StateRead,
    channel_id: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
) -> Result<(Vec<TagRow>, bool, Option<String>), Fail> {
    let limit = limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let prefix = match &channel_id {
        Some(channel) => format!("tagrank/c/{}/", hex_lower(channel.as_bytes())),
        None => "tagrank/g/".into(),
    };
    validate_rank_cursor(after.as_deref(), &prefix)?;
    let page = read.scan_page(
        prefix.as_bytes(),
        after.as_deref().map(str::as_bytes),
        limit,
    );
    let mut out = Vec::with_capacity(page.entries.len());
    for (key, value) in &page.entries {
        let rest = String::from_utf8_lossy(&key[prefix.len()..]);
        let Some((_, label)) = rest.split_once('/') else {
            continue;
        };
        let cat: TagCat =
            serde_json::from_slice(value).map_err(|e| Fail::new(FAIL_ROW_DECODE, e.to_string()))?;
        let posting_prefix = match &channel_id {
            Some(channel) => tag_channel_prefix(label, channel),
            None => tag_prefix(label),
        };
        let posting = read.scan_page(posting_prefix.as_bytes(), None, 1);
        let (_, value) = posting
            .entries
            .first()
            .ok_or_else(|| Fail::new(FAIL_ROW_DECODE, "tag catalog has no posting"))?;
        let last_seq = decode_tok(value)?.seq;
        out.push(TagRow {
            tag: label.into(),
            count: cat.count,
            last_seq,
        });
    }
    Ok((out, page.has_more, page.next_after))
}

/// the `TagSearch` query: every live message carrying EXACTLY `tag` (the
/// query normalizes like the indexer, so `#Rust` finds `#rust`), newest
/// first, clamped like search. one walk over the label's postings — a
/// channel scope narrows the scan prefix but the SCOPE ITSELF is the stored
/// ref's channel id, exactly like `Search`: the prefix alone also matches
/// sub-channels (`tag/{label}/g/` catches channel `g/0`). collects up to the
/// posting cap and ranks by time like `Search`.
pub(super) fn serve_tag_search(
    read: &impl StateRead,
    tag: &str,
    channel_id: Option<String>,
    after: Option<String>,
    limit: Option<usize>,
) -> Result<(Vec<MsgRow>, bool, Option<String>), Fail> {
    let label = normalize(tag.trim().trim_start_matches('#'));
    if label.is_empty() || label.chars().count() > MAX_TAG_CHARS || !label.chars().all(is_tag_char)
    {
        return Err(Fail::new(FAIL_BAD_REQUEST, "not a valid tag"));
    }
    let limit = limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let prefix = match &channel_id {
        Some(channel) => tag_channel_prefix(&label, channel),
        None => tag_prefix(&label),
    };
    validate_posting_cursor(after.as_deref(), &prefix, channel_id.is_some())?;
    let page = read.scan_page(
        prefix.as_bytes(),
        after.as_deref().map(str::as_bytes),
        limit,
    );
    let mut hits = Vec::with_capacity(page.entries.len());
    for (_, value) in &page.entries {
        let r = decode_tok(value)?;
        if let Some(bytes) = read.get(msg_key(&r.channel_id, r.seq).as_bytes()) {
            let row: MsgRow = serde_json::from_slice(&bytes)
                .map_err(|e| Fail::new(FAIL_ROW_DECODE, e.to_string()))?;
            hits.push(row);
        }
    }
    Ok((hits, page.has_more, page.next_after))
}

// ── extraction tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    fn para(text: &str) -> Vec<Block> {
        vec![Block::paragraph(text)]
    }

    #[test]
    fn extracts_basic_tags_in_appearance_order() {
        assert_eq!(
            labels(&para("shipping #beta today, then #alpha")),
            ["beta", "alpha"]
        );
    }

    #[test]
    fn extracts_non_ascii_underscore_and_hyphen_tags() {
        assert_eq!(
            labels(&para("#naïve support #snake_case #kebab-case")),
            ["naïve", "snake_case", "kebab-case"]
        );
    }

    #[test]
    fn start_of_text_and_punctuation_open_tags() {
        assert_eq!(labels(&para("#lead rest")), ["lead"]);
        assert_eq!(
            labels(&para("(#paren) ,#comma !#bang")),
            ["paren", "comma", "bang"]
        );
    }

    #[test]
    fn mid_word_hash_is_not_a_tag() {
        assert!(labels(&para("foo#bar issue#42")).is_empty());
    }

    #[test]
    fn url_fragments_entities_and_double_hash_are_not_tags() {
        assert!(labels(&para("see https://x.com/page#frag")).is_empty());
        assert!(labels(&para("see https://x.com/#frag")).is_empty());
        assert!(labels(&para("&#39; entity")).is_empty());
        assert!(labels(&para("##double")).is_empty());
    }

    #[test]
    fn bare_hash_and_heading_are_not_tags() {
        assert!(labels(&para("# heading")).is_empty());
        assert!(labels(&para("#")).is_empty());
        assert!(labels(&para("count # things")).is_empty());
    }

    #[test]
    fn code_blocks_never_carry_tags() {
        let blocks = vec![
            Block::Code {
                lang: Some("sh".into()),
                text: "#!/bin/sh\necho #nope".into(),
            },
            Block::paragraph("#yes"),
        ];
        assert_eq!(labels(&blocks), ["yes"]);
    }

    #[test]
    fn link_marked_spans_never_carry_tags() {
        let blocks = vec![Block::Paragraph(vec![
            Span {
                text: "https://x.com/#nope".into(),
                marks: vec![Mark::Link("https://x.com/#nope".into())],
            },
            Span::plain(" #yes"),
        ])];
        assert_eq!(labels(&blocks), ["yes"]);
    }

    #[test]
    fn quote_blocks_carry_tags_dividers_do_not() {
        let blocks = vec![Block::Quote(vec![Span::plain("#quoted")]), Block::Divider];
        assert_eq!(labels(&blocks), ["quoted"]);
    }

    #[test]
    fn normalizes_nfc_lowercase_and_dedups_labels() {
        // decomposed E + acute accent folds to the lowercase precomposed character.
        assert_eq!(normalize("E\u{301}"), "é");
        // three spellings, one label.
        assert_eq!(labels(&para("#Rust #RUST #rust")), ["rust"]);
    }

    #[test]
    fn tag_length_bounds() {
        let max = "a".repeat(MAX_TAG_CHARS);
        assert_eq!(labels(&para(&format!("#{max}"))), [max.as_str()]);
        // one char over: the whole run is rejected, not truncated — and the
        // run does not re-open a tag mid-way.
        assert!(labels(&para(&format!("#{max}b"))).is_empty());
        // non-ASCII text counts CHARS, not bytes.
        let accented = "é".repeat(MAX_TAG_CHARS);
        assert_eq!(labels(&para(&format!("#{accented}"))), [accented]);
    }

    #[test]
    fn digits_are_valid_tags() {
        assert_eq!(labels(&para("we are #1")), ["1"]);
    }

    #[test]
    fn caps_at_sixteen_distinct_labels() {
        let text: String = (0..20).map(|i| format!("#tag{i:02} ")).collect();
        let got = labels(&para(&text));
        assert_eq!(got.len(), MAX_TAGS_PER_MESSAGE);
        // first sixteen in appearance order survive.
        assert_eq!(got[0], "tag00");
        assert_eq!(got[15], "tag15");
    }

    #[test]
    fn consecutive_and_adjacent_tags() {
        // `#a#b`: the second # is mid-run — only `a` tags.
        assert_eq!(labels(&para("#a#b")), ["a"]);
        assert_eq!(labels(&para("#a #b")), ["a", "b"]);
    }

    #[test]
    fn reversed_seq_keys_order_newest_first() {
        let older = tag_key("t", "g", 1);
        let newer = tag_key("t", "g", 2);
        assert!(newer < older, "higher seq must sort first");
    }
}
