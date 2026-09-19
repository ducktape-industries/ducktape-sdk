//! shared text-index helpers for mapper views: tokenization and posting-list
//! intersection. domain-agnostic — a mapper decides its posting key shape;
//! this module only agrees on the convention that every token's postings
//! share a common prefix and an identical per-target suffix (the "rest"), so
//! AND-intersection is exact-key probes, never set merges. pure over
//! [`StateRead`], so decision cores use it natively and in the guest alike.

use std::collections::{BTreeMap, BTreeSet};

use crate::{MAX_SCAN_LIMIT, StateRead};

/// cap on distinct tokens indexed per text (alphabetical truncation beyond
/// it) — bounds the fan-out of a pathological message/block.
const MAX_TOKENS_PER_TEXT: usize = 256;
/// cap on how many tokens ONE QUERY intersects (alphabetical truncation beyond
/// it, like [`MAX_TOKENS_PER_TEXT`]). a text can be indexed under 256 tokens,
/// so a query could name that many and multiply its read cost by 256; a
/// truncated query reports [`Postings::capped`] instead.
pub const MAX_QUERY_TOKENS: usize = 16;
/// how many postings of one token a search READS. an out-of-scope posting
/// costs a read but not a result slot, so a narrow scope reaches past the
/// [`DEFAULT_POSTING_CAP`] nearest postings of a common token — never past
/// this, though: one query reads at most `MAX_QUERY_TOKENS * MAX_POSTING_SCAN`.
pub const MAX_POSTING_SCAN: usize = 1 << 14;
/// default cap on one token's contribution to a search. in [`intersect_prefix`]
/// it bounds the IN-SCOPE postings kept — the newest by the caller's key, since
/// the cap is applied after the scope filter and the ordering; a caller walking
/// its own postings bounds those instead. either way results beyond it are out
/// of reach, which [`intersect_prefix`] reports in [`Postings::capped`].
pub const DEFAULT_POSTING_CAP: usize = 4096;

/// lowercase alphanumeric tokens of `text`, deduplicated, single-char noise
/// dropped, capped at [`MAX_TOKENS_PER_TEXT`].
pub fn tokens(text: &str) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = text
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_string)
        .collect();
    while set.len() > MAX_TOKENS_PER_TEXT {
        set.pop_last();
    }
    set
}

/// one intersection survivor: the per-target `rest` (posting key minus its
/// `{key_ns}{token}/` head) and one posting value for that target.
pub struct PostingHit {
    pub rest: String,
    pub value: Vec<u8>,
}

/// what one intersection found, and whether a bound bit while finding it.
#[derive(Default)]
pub struct Postings {
    /// the surviving targets, ordered by the caller's key, greatest first.
    pub hits: Vec<PostingHit>,
    /// some matching postings are out of reach: the query named more than
    /// [`MAX_QUERY_TOKENS`] tokens, a token had more in-scope postings than
    /// the cap, or a token's postings ran past [`MAX_POSTING_SCAN`]. a caller
    /// that can say so turns this into a "narrow the query" hint.
    pub capped: bool,
}

/// the target a posting key addresses: everything after its `{key_ns}{token}/`
/// head. postings share `key_ns` (e.g. `tok/`) and store the token as the first
/// `/`-delimited segment after it, so the `rest` is the mapper's own target
/// suffix — `{channel}/{seq}`, `{doc}/{block}`, a bare `{block}`, whatever it
/// chose. the `/` after the token is found at the byte level: `/` (0x2f) is
/// never a utf-8 continuation byte, so a non-ascii token or target is safe.
fn target_rest(key: &[u8], ns_len: usize) -> Option<String> {
    let after_ns = key.get(ns_len..)?;
    let slash = after_ns.iter().position(|&b| b == b'/')?;
    Some(String::from_utf8_lossy(&after_ns[slash + 1..]).into_owned())
}

/// AND-intersect token PREFIX matches. each query token matches any indexed
/// token that STARTS WITH it (search-as-you-type: `tes` finds `testing`); a
/// target survives only when EVERY query token has some such match on it.
///
/// `key_ns` is the shared posting namespace (`tok/`); `tokens` are the query's
/// prefixes. for each token this scans `{key_ns}{token}` and folds its postings
/// down to the distinct set of targets, then intersects those sets. a target
/// that carries several matching words (`test`, `tester`, `testing` all match
/// `tes`) collapses to ONE hit — the map dedups by target.
///
/// `scope` is the caller's scope filter AND its ordering in one: given a
/// posting value (whose stored ref names the target in full) it answers `None`
/// for a posting outside the requested channel/doc/page, or `Some(key)` for one
/// inside it. postings are filtered by it, ordered by that key and only THEN
/// capped at `cap` per token, so a scope's newest hits survive even when
/// postings outside it sort first in key order. hits come back greatest key
/// first; a caller that wants newest-first picks a key that grows with time.
pub fn intersect_prefix<K: Ord>(
    reader: &impl StateRead,
    key_ns: &str,
    tokens: &[String],
    cap: usize,
    scope: impl Fn(&[u8]) -> Option<K>,
) -> Postings {
    if tokens.is_empty() {
        return Postings::default();
    }
    let ns_len = key_ns.len();
    let mut capped = tokens.len() > MAX_QUERY_TOKENS;
    let mut acc: Option<BTreeMap<String, (K, Vec<u8>)>> = None;
    for token in tokens.iter().take(MAX_QUERY_TOKENS) {
        let scan_prefix = format!("{key_ns}{token}");
        let mut targets: BTreeMap<String, (K, Vec<u8>)> = BTreeMap::new();
        let mut cursor: Option<String> = None;
        let mut read = 0usize;
        'walk: loop {
            let page = reader.scan_page(
                scan_prefix.as_bytes(),
                cursor.as_deref().map(str::as_bytes),
                MAX_SCAN_LIMIT,
            );
            for (key, value) in &page.entries {
                if read == MAX_POSTING_SCAN {
                    capped = true;
                    break 'walk;
                }
                read += 1;
                // out of scope: a read, but not a slot in the result.
                let Some(order) = scope(value) else { continue };
                if let Some(rest) = target_rest(key, ns_len) {
                    targets
                        .entry(rest)
                        .or_insert_with(|| (order, value.clone()));
                }
            }
            match page.next_after {
                Some(next) if page.has_more => cursor = Some(next),
                _ => break,
            }
        }
        if targets.len() > cap {
            capped = true;
            let mut ranked: Vec<(String, (K, Vec<u8>))> = targets.into_iter().collect();
            ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0));
            ranked.truncate(cap);
            targets = ranked.into_iter().collect();
        }
        acc = Some(match acc {
            None => targets,
            // keep only targets present for this token too; carry the earlier
            // value (any posting of a target names it identically).
            Some(prev) => prev
                .into_iter()
                .filter(|(target, _)| targets.contains_key(target))
                .collect(),
        });
        // an empty intersection can only stay empty — stop scanning.
        if acc.as_ref().is_some_and(BTreeMap::is_empty) {
            break;
        }
    }
    let mut ranked: Vec<(String, (K, Vec<u8>))> = acc.unwrap_or_default().into_iter().collect();
    ranked.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    Postings {
        hits: ranked
            .into_iter()
            .map(|(rest, (_, value))| PostingHit { rest, value })
            .collect(),
        capped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_lowercase_dedup_and_drop_noise() {
        let toks = tokens("Hello, hello WORLD! a b1 -- code_path");
        let want: BTreeSet<String> = ["hello", "world", "b1", "code", "path"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(toks, want);
    }

    #[test]
    fn tokens_cap_is_enforced() {
        let text: String = (0..600).map(|i| format!("tok{i:03} ")).collect();
        assert_eq!(tokens(&text).len(), MAX_TOKENS_PER_TEXT);
    }

    #[test]
    fn intersection_is_exact_over_a_map() {
        let mut map = std::collections::BTreeMap::new();
        crate::apply_to_map(
            &mut map,
            vec![
                ("tok/testing/ch/0001".into(), Some(b"a".to_vec())),
                ("tok/testing/ch/0002".into(), Some(b"b".to_vec())),
                ("tok/world/ch/0001".into(), Some(b"a".to_vec())),
            ],
        );

        let both: Vec<String> = ["tes".to_string(), "wor".to_string()].to_vec();
        let found = intersect_prefix(&map, "tok/", &both, DEFAULT_POSTING_CAP, |_| Some(()));
        assert_eq!(found.hits.len(), 1, "only ch/0001 carries both tokens");
        assert_eq!(found.hits[0].rest, "ch/0001");
        assert!(!found.capped, "three postings are under every bound");

        let one = ["tes".to_string()];
        assert_eq!(
            intersect_prefix(&map, "tok/", &one, DEFAULT_POSTING_CAP, |_| Some(()))
                .hits
                .len(),
            2
        );
    }

    /// a posting value here is `{channel}:{time}`; the scope keeps one channel
    /// and orders by time, the way chat's and pages' refs do.
    fn scoped(want: &'static str) -> impl Fn(&[u8]) -> Option<u64> {
        move |value| {
            let (channel, time) = std::str::from_utf8(value).ok()?.split_once(':')?;
            (channel == want).then(|| time.parse().expect("test posting carries a time"))
        }
    }

    #[test]
    fn the_cap_bites_after_the_scope_filter_and_the_order() {
        let mut map = std::collections::BTreeMap::new();
        crate::apply_to_map(
            &mut map,
            vec![
                // out of scope, and first in KEY order: capping before the
                // filter would spend the whole budget here.
                ("tok/testing/a1".into(), Some(b"ch0:97".to_vec())),
                ("tok/testing/a2".into(), Some(b"ch0:98".to_vec())),
                ("tok/testing/a3".into(), Some(b"ch0:99".to_vec())),
                ("tok/testing/b1".into(), Some(b"ch1:1".to_vec())),
                ("tok/testing/b2".into(), Some(b"ch1:2".to_vec())),
                ("tok/testing/b3".into(), Some(b"ch1:3".to_vec())),
                ("tok/testing/b4".into(), Some(b"ch1:4".to_vec())),
            ],
        );

        let one = ["tes".to_string()];
        let found = intersect_prefix(&map, "tok/", &one, 2, scoped("ch1"));
        let rests: Vec<&str> = found.hits.iter().map(|h| h.rest.as_str()).collect();
        assert_eq!(
            rests,
            ["b4", "b3"],
            "the newest in-scope hits, newest first"
        );
        assert!(
            found.capped,
            "two of the four in-scope hits are out of reach"
        );
    }

    #[test]
    fn a_query_intersects_at_most_max_query_tokens() {
        let mut map = std::collections::BTreeMap::new();
        let mut writes: Vec<(String, Option<Vec<u8>>)> = (0..MAX_QUERY_TOKENS)
            .map(|i| (format!("tok/t{i:02}/x"), Some(b"ch1:1".to_vec())))
            .collect();
        // a token that matches nothing, sorting after all the others: it is
        // proof the intersection never reached it.
        writes.push(("tok/zz/other".into(), Some(b"ch0:1".to_vec())));
        crate::apply_to_map(&mut map, writes);

        let mut query: Vec<String> = (0..MAX_QUERY_TOKENS).map(|i| format!("t{i:02}")).collect();
        query.push("zz".to_string());
        let found = intersect_prefix(&map, "tok/", &query, DEFAULT_POSTING_CAP, scoped("ch1"));
        assert_eq!(found.hits.len(), 1, "`zz` was never intersected");
        assert_eq!(found.hits[0].rest, "x");
        assert!(found.capped, "the query named more tokens than the cap");
    }
}
