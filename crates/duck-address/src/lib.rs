//! the `duck://` address: one grammar for naming a thing on a ducktape
//! network, parsed in one place.
//!
//! ```text
//! duck://<label>-<salt>/<module>/<module-path…>
//! duck://dognet-b5b6ea90/forge/<owner>/<repo>
//! ```
//!
//! THE AUTHORITY IS THE CHAIN ID. The workspace registry keys a network
//! `<label>#<salt>` (core's `mint_chain_id` spells it, the salt being hex digits
//! of the genesis digest: 8 today, and the owner may lengthen them, so any even
//! count from 8 to 64 reads), and `#` starts a URL fragment, so an address
//! writes the same pair with `-`. The salt is the match key; the label is
//! display. Whether a label AGREES with the registry is not a question this
//! crate can answer — that needs `~/.ducktape/registry.json` and this crate
//! reads nothing — so `label_mismatch` is core's refusal, not ours. A
//! salt-only or label-only authority is refused: the address carries the chain
//! id whole, exactly as the registry keys it.
//!
//! THE FIRST PATH SEGMENT NAMES A MODULE — its registered id, one of
//! [`MODULES`] — and everything after it belongs to that module. [`Address`]
//! keeps the tail as segments and interprets none of it. What a tail MEANS is a
//! typed address, one public module here per module id ([`pages::PageAddress`],
//! [`chat::MessageAddress`], [`runs::RunAddress`], [`forge::ForgeRepoAddress`]
//! and [`forge::ForgeLocator`]): they ship in this crate and not in each
//! module's wire crate because a view must be able to name a thing without
//! linking that module's signing and identity graph, which chat-wire, runs-wire
//! and forge-wire reach and a wasm32 component cannot build. Each module's wire
//! crate re-exports its own tail from the path it always had, so a module's
//! name rule still has one home. files is the exception: `files-wire`'s
//! `FileAddress` canonicalises its path with duckfs's own rule, which this
//! dependency-free crate cannot link, and files-wire builds for wasm32 anyway.
//!
//! THE TAIL CARRIES ANY NAME, IN EXACTLY ONE SPELLING. A module names files
//! and ids that are not `[a-z0-9._-]` (`보고서 Final.pdf`, `Blk_7`), so a
//! segment after the module may hold any text but `/`, NUL, `.` and `..`.
//! [`Address::path`] holds it decoded; `Display` writes a byte literally iff it
//! is RFC 3986 unreserved (`A-Z a-z 0-9 - . _ ~`) and every other byte as `%XX`
//! in uppercase hex; `parse` refuses every other spelling of the same name
//! (lowercase hex, an escaped unreserved byte, a literal space) instead of
//! normalising it. Two spellings of one address would be two cache keys, two
//! lock-file lines and two registry lookups.
//!
//! The chain id and the module segment are never encoded and are lowercase,
//! and nothing here case-folds anything: `A` and `a` in a tail segment are two
//! names.
//!
//! See ducktape#2616 (design note v3) for why the grammar is this.

use refusal_class::INVALID_INPUT;

pub mod chat;
pub mod forge;
pub mod pages;
pub mod runs;

/// the scheme, spelled once.
const SCHEME: &str = "duck://";

/// how many hex digits a salt may be — `mint_chain_id` takes 4 bytes off the
/// genesis digest today and writes them as 8 lowercase hex; the owner may take
/// more later, up to the whole 32-byte digest, so the length is a range and
/// not a constant. At least 8, so `my-cafe` is a label and not `my` salted
/// `cafe`.
const SALT_HEX: std::ops::RangeInclusive<usize> = 8..=64;

/// the modules that claim a `duck://` name, by registered module id — never an
/// alias. A name is refused until its module claims it here, so an address for
/// a module nobody has built cannot be read as one for a module that exists.
pub const MODULES: [&str; 5] = ["forge", "pages", "chat", "files", "runs"];

/// the network, as the workspace registry keys it: `<label>#<salt>`.
///
/// `salt` is the match key (raw bytes, so a comparison cannot be fooled by a
/// spelling); `label` is what a human reads. Both are public because a
/// consumer resolving an address against the registry needs both halves — this
/// is a pair, not an invariant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainId {
    pub label: String,
    pub salt: Vec<u8>,
}

impl ChainId {
    /// the URL spelling of the same pair: `<label>-<salt>`, what a `duck://`
    /// address puts in its authority.
    pub fn authority(&self) -> String {
        format!("{}-{}", self.label, self.salt_hex())
    }

    /// the salt's lowercase hex digits, two per byte — the spelling the
    /// registry, a node's status and a push certificate all use. ONE home for
    /// it, so nothing re-derives the padding.
    pub fn salt_hex(&self) -> String {
        self.salt.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

/// the registry's spelling.
impl std::fmt::Display for ChainId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}#{}", self.label, self.salt_hex())
    }
}

impl std::str::FromStr for ChainId {
    type Err = Refused;

    /// either spelling: `<label>#<salt>` as the registry writes it, or
    /// `<label>-<salt>` as an address writes it. The one rule for a network
    /// name, wherever one is read.
    ///
    /// Split from the RIGHT in both cases: a label may itself contain `-`, and
    /// `node init --name` validates nothing, so only the LAST separator is the
    /// minted one. A string carrying `#` is read as the registry spelling —
    /// `#` cannot occur in an address at all.
    fn from_str(text: &str) -> Result<Self, Refused> {
        lowercase(text)?;
        let split = match text.contains('#') {
            true => text.rsplit_once('#'),
            false => text.rsplit_once('-'),
        };
        let Some((label, salt)) = split else {
            return Err(incomplete(text));
        };
        let labelled = !label.is_empty()
            && label
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'));
        let salted = SALT_HEX.contains(&salt.len())
            && salt.len() % 2 == 0
            && salt.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !labelled || !salted {
            return Err(incomplete(text));
        }
        let Ok(salt) = (0..salt.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&salt[at..at + 2], 16))
            .collect()
        else {
            return Err(incomplete(text));
        };
        Ok(ChainId {
            label: label.to_string(),
            salt,
        })
    }
}

/// a parsed `duck://` address.
///
/// `path` is everything after the module segment, DECODED — `보고서 Final.pdf`,
/// never `%EB%B3%B4…` — and what it means is the module's question.
/// `Address::parse` and [`Display`](std::fmt::Display) round-trip both ways: a
/// parsed address prints the string it came from, and that string is the only
/// one that parses to it.
///
/// The fields are public so a consumer can read them, and a struct literal
/// checks nothing: a hand-built segment carrying `/`, or an unclaimed module,
/// prints a string that does not parse back. [`Address::parse`] and
/// [`Address::new`] are the only ways to get a value that round-trips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub chain: ChainId,
    pub module: String,
    pub path: Vec<String>,
}

impl Address {
    pub fn parse(text: &str) -> Result<Self, Refused> {
        let rest = text.strip_prefix(SCHEME).ok_or_else(|| {
            Refused::new(
                INVALID_INPUT,
                format!("A ducktape address starts with `{SCHEME}`, and `{text}` does not."),
            )
        })?;
        // a query and a fragment terminate a URL wherever they appear, so they
        // are looked for across the whole of it; userinfo and a port are parts
        // of an authority and are looked for there.
        if rest.contains('?') || rest.contains('#') {
            return Err(extra(text));
        }
        let Some((authority, path)) = rest.split_once('/') else {
            return Err(empty(text));
        };
        if authority.contains('@') || authority.contains(':') {
            return Err(extra(text));
        }
        if authority.is_empty() || path.is_empty() {
            return Err(empty(text));
        }
        let chain: ChainId = authority.parse()?;
        let mut segments = Vec::new();
        for segment in path.split('/') {
            if segment.is_empty() {
                return Err(empty(text));
            }
            segments.push(segment);
        }
        let module = segments.remove(0).to_string();
        claimed(&module)?;
        Ok(Address {
            chain,
            module,
            path: segments.into_iter().map(decode).collect::<Result<_, _>>()?,
        })
    }

    /// an address from its parts, checked by the rules [`Address::parse`]
    /// applies to a chain id, a module segment and a decoded tail segment — the
    /// constructor a module's typed address prints itself through.
    pub fn new(chain: ChainId, module: &str, path: Vec<String>) -> Result<Self, Refused> {
        chain.authority().parse::<ChainId>()?;
        claimed(module)?;
        let address = Address {
            chain,
            module: module.to_string(),
            path,
        };
        if address.path.iter().any(String::is_empty) {
            return Err(empty(&address.to_string()));
        }
        for segment in &address.path {
            named(segment, &encode(segment))?;
        }
        Ok(address)
    }
}

/// a tail segment that spells a number: decimal, no sign, no leading zero (`0`
/// itself aside), within `u64`. One spelling, so `7`, `07` and `+7` are not
/// three addresses of one thing; a module that numbers its tail reads it here
/// and words its own refusal.
pub fn number(segment: &str) -> Option<u64> {
    let canonical = !segment.is_empty()
        && segment.bytes().all(|byte| byte.is_ascii_digit())
        && (segment == "0" || !segment.starts_with('0'));
    match canonical {
        true => segment.parse().ok(),
        false => None,
    }
}

/// the module segment: lowercase, and one of [`MODULES`].
fn claimed(module: &str) -> Result<(), Refused> {
    lowercase(module)?;
    if !MODULES.contains(&module) {
        let claimed = MODULES.map(|claimed| format!("`{claimed}`")).join(", ");
        return Err(Refused::new(
            INVALID_INPUT,
            format!("`{module}` names no ducktape module; the module segment is one of {claimed}."),
        ));
    }
    Ok(())
}

impl std::fmt::Display for Address {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{SCHEME}{}/{}",
            self.chain.authority(),
            self.module
        )?;
        for segment in &self.path {
            write!(formatter, "/{}", encode(segment))?;
        }
        Ok(())
    }
}

/// a name's one spelling as a path segment.
fn encode(name: &str) -> String {
    let mut spelled = String::with_capacity(name.len());
    for byte in name.bytes() {
        match unreserved(byte) {
            true => spelled.push(byte as char),
            false => spelled.push_str(&format!("%{byte:02X}")),
        }
    }
    spelled
}

/// RFC 3986's unreserved bytes: the only ones a path segment writes literally.
fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// one tail segment, from its one spelling to the name it carries. Every other
/// spelling of the same name is refused, never normalised.
fn decode(segment: &str) -> Result<String, Refused> {
    let refuse = |rule: &str| {
        Err(Refused::new(
            INVALID_INPUT,
            format!("A duck:// path segment {rule}, and `{segment}` does not."),
        ))
    };
    let mut decoded = Vec::with_capacity(segment.len());
    let mut bytes = segment.bytes();
    while let Some(byte) = bytes.next() {
        if unreserved(byte) {
            decoded.push(byte);
            continue;
        }
        if byte != b'%' {
            return refuse("writes only [A-Za-z0-9._~-] literally and every other byte as `%XX`");
        }
        let (high, low) = match (bytes.next(), bytes.next()) {
            (Some(high), Some(low)) if high.is_ascii_hexdigit() && low.is_ascii_hexdigit() => {
                (high, low)
            }
            _ => return refuse("writes `%` only to open a `%XX` escape of two hex digits"),
        };
        if high.is_ascii_lowercase() || low.is_ascii_lowercase() {
            return refuse("writes a `%XX` escape in uppercase hex");
        }
        // both are 0-9 or A-F here
        let nibble = |digit: u8| match digit.is_ascii_digit() {
            true => digit - b'0',
            false => digit - b'A' + 10,
        };
        let escaped = nibble(high) << 4 | nibble(low);
        if unreserved(escaped) {
            return refuse("writes [A-Za-z0-9._~-] literally, never as `%XX`");
        }
        decoded.push(escaped);
    }
    let Ok(decoded) = String::from_utf8(decoded) else {
        return refuse("decodes to UTF-8 text");
    };
    named(&decoded, segment)?;
    Ok(decoded)
}

/// the names no tail segment carries, however it was built: one with `/` or
/// NUL in it, `.` and `..`. The refusal quotes `spelling`, the segment as an
/// address writes it.
fn named(name: &str, spelling: &str) -> Result<(), Refused> {
    let refuse = |rule: &str| {
        Err(Refused::new(
            INVALID_INPUT,
            format!("A duck:// path segment {rule}, and `{spelling}` does not."),
        ))
    };
    if name.contains(['/', '\0']) {
        return refuse("decodes to a name with no `/` and no NUL in it");
    }
    if name == "." || name == ".." {
        return refuse("names something other than `.` or `..`");
    }
    Ok(())
}

/// why an address was refused: a token to BRANCH on and a sentence to SHOW.
///
/// The shape boards-wire settled on, for the reason it settled on it: a
/// refusal that is only prose has to be matched by prose, and the next edit to
/// the wording breaks the match.
///
/// `reason` names a CLASS and not a site (a [`refusal_class`] constant): every
/// rule an address breaks, here or in a module's half of the path, refuses as
/// [`INVALID_INPUT`], because a caller fixes the address whatever rule it
/// broke. `sentence` is a complete sentence a developer can act on: it names
/// the rule and quotes what was refused.
///
/// `new` is public: a module validating its own half of a path (the typed
/// tails here, files-wire's `FileAddress`) refuses in the same shape rather
/// than inventing a second error type for the same grammar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub reason: &'static str,
    pub sentence: String,
}

impl Refused {
    pub fn new(reason: &'static str, sentence: impl Into<String>) -> Self {
        Self {
            reason,
            sentence: sentence.into(),
        }
    }
}

/// So something that only wants to SHOW the refusal writes `{refused}` — the
/// token is for branching, not for reading.
impl std::fmt::Display for Refused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.sentence)
    }
}

fn lowercase(text: &str) -> Result<(), Refused> {
    match text.bytes().any(|byte| byte.is_ascii_uppercase()) {
        true => Err(Refused::new(
            INVALID_INPUT,
            format!(
                "A duck:// chain id and module segment are lowercase and nothing case-folds them, but `{text}` carries an uppercase letter."
            ),
        )),
        false => Ok(()),
    }
}

fn incomplete(text: &str) -> Refused {
    Refused::new(
        INVALID_INPUT,
        format!(
            "A duck:// authority is the whole chain id `<label>-<salt>` (the registry's `<label>#<salt>`), with `<label>` matching [a-z0-9-] and `<salt>` an even count of {} to {} lowercase hex digits; `{text}` is not one.",
            SALT_HEX.start(),
            SALT_HEX.end(),
        ),
    )
}

fn extra(text: &str) -> Refused {
    Refused::new(
        INVALID_INPUT,
        format!(
            "A duck:// address carries no credentials, port, query or fragment — the node and its credential come from the workspace registry — but `{text}` carries one."
        ),
    )
}

fn empty(text: &str) -> Refused {
    Refused::new(
        INVALID_INPUT,
        format!(
            "A duck:// address is `duck://<label>-<salt>/<module>/…`, with an authority and at least the module segment, none of them empty; `{text}` leaves one empty."
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> ChainId {
        "dognet-b5b6ea90".parse().expect("a chain id parses")
    }

    #[test]
    fn parses_the_canonical_form() {
        let address =
            Address::parse("duck://dognet-b5b6ea90/forge/alice/my-crate").expect("parses");
        assert_eq!(address.chain, chain());
        assert_eq!(address.chain.salt, [0xb5, 0xb6, 0xea, 0x90]);
        assert_eq!(address.module, "forge");
        assert_eq!(address.path, ["alice", "my-crate"]);
    }

    #[test]
    fn round_trips_through_display() {
        for text in [
            "duck://dognet-b5b6ea90/forge/alice/my-crate",
            "duck://my-long-net-00000000/forge/alice/my.crate_1",
            "duck://dognet-b5b6ea90/forge",
            "duck://dognet-b5b6ea90/forge/a/b/c/d",
        ] {
            let address = Address::parse(text).expect("parses");
            assert_eq!(address.to_string(), text);
            assert_eq!(Address::parse(&address.to_string()), Ok(address));
        }
    }

    fn forge(path: &[&str]) -> Address {
        Address {
            chain: chain(),
            module: "forge".to_string(),
            path: path.iter().map(|segment| segment.to_string()).collect(),
        }
    }

    /// a tail segment carries any name, and exactly one string spells it.
    #[test]
    fn a_segment_carries_any_name_in_one_spelling() {
        for (path, text) in [
            (
                &["files", "shared", "보고서 Final.pdf"][..],
                "duck://dognet-b5b6ea90/forge/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf",
            ),
            (&["a~b"], "duck://dognet-b5b6ea90/forge/a~b"),
            (
                &["pages", "Blk_7"],
                "duck://dognet-b5b6ea90/forge/pages/Blk_7",
            ),
            (&["%"], "duck://dognet-b5b6ea90/forge/%25"),
            (&["a?b#c"], "duck://dognet-b5b6ea90/forge/a%3Fb%23c"),
            (&["..."], "duck://dognet-b5b6ea90/forge/..."),
        ] {
            let address = forge(path);
            assert_eq!(address.to_string(), text);
            assert_eq!(Address::parse(text), Ok(address));
        }
    }

    /// `A` and `a` are two names: nothing folds one into the other.
    #[test]
    fn case_in_a_segment_is_kept() {
        let upper = Address::parse("duck://dognet-b5b6ea90/forge/A").expect("parses");
        let lower = Address::parse("duck://dognet-b5b6ea90/forge/a").expect("parses");
        assert_eq!(upper.path, ["A"]);
        assert_ne!(upper, lower);
    }

    /// every ASCII byte and a few multi-byte names, as a one-segment tail: the
    /// printed address uses only the canonical alphabet and parses back to the
    /// same name, or the name is one no segment may carry and is refused.
    #[test]
    fn every_name_prints_canonically_and_parses_back() {
        let ascii = (0x01..=0x7F_u8).map(|byte| (byte as char).to_string());
        let wider = ["보고서 Final.pdf", "é", "日本語", "🦆", "a b+c", "%41"].map(String::from);
        for name in ascii.chain(wider) {
            let address = forge(&[&name]);
            let text = address.to_string();
            if name == "/" || name == "." {
                assert!(Address::parse(&text).is_err(), "{text}");
                continue;
            }
            assert!(
                text.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._~%/:-".contains(&byte)),
                "{text}"
            );
            assert_eq!(Address::parse(&text), Ok(address), "{name:?}");
        }
    }

    #[test]
    fn a_label_may_carry_dashes_and_the_last_one_splits() {
        let address = Address::parse("duck://my-long-net-b5b6ea90/forge/a/b").expect("parses");
        assert_eq!(address.chain.label, "my-long-net");
        assert_eq!(address.chain.salt_hex(), "b5b6ea90");
    }

    #[test]
    fn a_chain_id_reads_and_prints_both_spellings() {
        let registry: ChainId = "dognet#b5b6ea90".parse().expect("parses");
        assert_eq!(registry, chain());
        assert_eq!(registry.to_string(), "dognet#b5b6ea90");
        assert_eq!(registry.authority(), "dognet-b5b6ea90");
    }

    /// every refusal is one class (fix the address), so the sentence is what
    /// tells the rules apart.
    fn refused(text: &str) -> String {
        sentence(Address::parse(text))
    }

    fn sentence<T: std::fmt::Debug>(parsed: Result<T, Refused>) -> String {
        let refused = parsed.expect_err("refused");
        assert_eq!(refused.reason, INVALID_INPUT);
        refused.sentence
    }

    const UPPERCASE: &str = "are lowercase and nothing case-folds them";
    const NO_SCHEME: &str = "starts with `duck://`";
    const EXTRA: &str = "carries no credentials, port, query or fragment";
    const EMPTY: &str = "none of them empty";
    const AUTHORITY: &str = "authority is the whole chain id";
    const NO_MODULE: &str = "names no ducktape module";
    const LITERAL: &str = "writes only [A-Za-z0-9._~-] literally";
    const ESCAPE: &str = "writes `%` only to open a `%XX` escape of two hex digits";
    const LOWER_HEX: &str = "writes a `%XX` escape in uppercase hex";
    const ESCAPED_UNRESERVED: &str = "writes [A-Za-z0-9._~-] literally, never as `%XX`";
    const UTF8: &str = "decodes to UTF-8 text";
    const SLASH_OR_NUL: &str = "decodes to a name with no `/` and no NUL in it";
    const DOT: &str = "names something other than `.` or `..`";

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        for (text, rule) in [
            ("duck://Dognet-b5b6ea90/forge/a/b", UPPERCASE),
            ("duck://dognet-B5B6EA90/forge/a/b", UPPERCASE),
            ("https://dognet-b5b6ea90/forge/a/b", NO_SCHEME),
            ("duck:/dognet-b5b6ea90/forge/a/b", NO_SCHEME),
            ("duck://user@dognet-b5b6ea90/forge/a", EXTRA),
            ("duck://dognet-b5b6ea90:443/forge/a", EXTRA),
            ("duck://dognet-b5b6ea90/forge/a?rev=1", EXTRA),
            ("duck://dognet-b5b6ea90/forge/a#head", EXTRA),
            ("duck://dognet-b5b6ea90", EMPTY),
            ("duck://dognet-b5b6ea90/", EMPTY),
            ("duck:///forge/a/b", EMPTY),
            ("duck://dognet-b5b6ea90/forge//b", EMPTY),
            ("duck://b5b6ea90/forge/a/b", AUTHORITY),
            ("duck://dognet/forge/a/b", AUTHORITY),
            ("duck://dognet-b5b6ea9/forge/a/b", AUTHORITY),
            ("duck://dognet-b5b6ea90z/forge/a/b", AUTHORITY),
            ("duck://-b5b6ea90/forge/a/b", AUTHORITY),
            ("duck://dognet-b5b6ea90/Forge/a/b", UPPERCASE),
            ("duck://dognet-b5b6ea90/gateway/a/b", NO_MODULE),
            ("duck://dognet-b5b6ea90/forge/a b", LITERAL),
            ("duck://dognet-b5b6ea90/forge/a+b", LITERAL),
            ("duck://dognet-b5b6ea90/forge/a:b", LITERAL),
            ("duck://dognet-b5b6ea90/forge/a@b", LITERAL),
            ("duck://dognet-b5b6ea90/forge/보고서", LITERAL),
            ("duck://dognet-b5b6ea90/forge/a%", ESCAPE),
            ("duck://dognet-b5b6ea90/forge/a%4", ESCAPE),
            ("duck://dognet-b5b6ea90/forge/%G1", ESCAPE),
            ("duck://dognet-b5b6ea90/forge/%%41", ESCAPE),
            ("duck://dognet-b5b6ea90/forge/%e4", LOWER_HEX),
            ("duck://dognet-b5b6ea90/forge/%Ea", LOWER_HEX),
            ("duck://dognet-b5b6ea90/forge/%41", ESCAPED_UNRESERVED),
            ("duck://dognet-b5b6ea90/forge/%7E", ESCAPED_UNRESERVED),
            ("duck://dognet-b5b6ea90/forge/%2E%2E", ESCAPED_UNRESERVED),
            ("duck://dognet-b5b6ea90/forge/%FF", UTF8),
            ("duck://dognet-b5b6ea90/forge/%EB%B3", UTF8),
            ("duck://dognet-b5b6ea90/forge/a%2Fb", SLASH_OR_NUL),
            ("duck://dognet-b5b6ea90/forge/a%00b", SLASH_OR_NUL),
            ("duck://dognet-b5b6ea90/forge/.", DOT),
            ("duck://dognet-b5b6ea90/forge/a/../b", DOT),
        ] {
            let sentence = refused(text);
            assert!(sentence.contains(rule), "{text}: {sentence}");
        }
    }

    /// a segment refusal quotes the segment as written, not the whole address.
    #[test]
    fn a_segment_refusal_quotes_the_segment() {
        assert!(refused("duck://dognet-b5b6ea90/forge/ok/%e4").ends_with("and `%e4` does not."));
    }

    /// the app's page origin is a different address family (a dotted host, no
    /// chain id) and must not parse as one of these — ducktape#2616 §4 moves
    /// it onto this grammar in its own unit, with no compat window.
    #[test]
    fn the_app_page_origin_is_not_this_grammar() {
        for text in ["duck://app.alice.duck/forge/a", "duck://page/00ff"] {
            assert!(refused(text).contains(AUTHORITY), "{text}");
        }
    }

    /// the salt is any even count of 8 to 64 lowercase hex digits, so the owner
    /// can lengthen it without a new parser; both spellings read and print.
    #[test]
    fn a_salt_is_any_even_length_from_8_to_64() {
        for salt in ["b5b6ea90", &"0123456789abcdef".repeat(4)] {
            let registry = format!("dognet#{salt}");
            let chain: ChainId = registry.parse().expect("parses");
            assert_eq!(chain.salt_hex(), salt);
            assert_eq!(chain.to_string(), registry);
            let authority = format!("my-net-{salt}");
            let chain: ChainId = authority.parse().expect("parses");
            assert_eq!(chain.authority(), authority);
            let text = format!("duck://my-net-{salt}/forge/alice/my-crate");
            assert_eq!(Address::parse(&text).expect("parses").to_string(), text);
        }
        for text in [
            "dognet-b5b6ea",
            "dognet-b5b6ea90a",
            &format!("dognet-{}", "ab".repeat(33)),
        ] {
            assert!(
                sentence(text.parse::<ChainId>()).contains(AUTHORITY),
                "{text}"
            );
        }
        assert!(sentence("dognet-B5B6EA90".parse::<ChainId>()).contains(UPPERCASE));
    }

    /// five modules claim a name; a sixth is refused by name, and the sentence
    /// says which five there are.
    #[test]
    fn five_modules_claim_their_names() {
        for module in MODULES {
            let text = format!("duck://dognet-b5b6ea90/{module}/a");
            assert_eq!(Address::parse(&text).expect("parses").module, module);
        }
        let refused = refused("duck://dognet-b5b6ea90/boards/a");
        assert!(
            refused.starts_with("`boards` names no ducktape module")
                && refused.ends_with("one of `forge`, `pages`, `chat`, `files`, `runs`."),
            "{refused}"
        );
    }

    /// a struct literal checks nothing; `new` checks what `parse` checks, so a
    /// value from `new` prints a string that parses back to it.
    #[test]
    fn new_refuses_what_parse_refuses() {
        let new = |module: &str, segment: &str| {
            sentence(Address::new(chain(), module, vec![segment.to_string()]))
        };
        for (segment, rule) in [
            ("a/b", SLASH_OR_NUL),
            ("a\0b", SLASH_OR_NUL),
            (".", DOT),
            ("..", DOT),
            ("", EMPTY),
        ] {
            assert!(new("forge", segment).contains(rule), "{segment:?}");
        }
        assert!(new("forge", "a/b").ends_with("and `a%2Fb` does not."));
        assert!(new("gateway", "a").contains(NO_MODULE));
        assert!(new("Forge", "a").contains(UPPERCASE));
        let unsalted = ChainId {
            label: "dognet".to_string(),
            salt: vec![0xb5],
        };
        assert!(sentence(Address::new(unsalted, "forge", vec![])).contains(AUTHORITY));
        let built = Address::new(chain(), "files", vec!["보고서 Final.pdf".to_string()])
            .expect("a legal name");
        assert_eq!(Address::parse(&built.to_string()), Ok(built));
    }

    #[test]
    fn a_number_has_one_spelling() {
        for (segment, parsed) in [
            ("0", Some(0)),
            ("7", Some(7)),
            ("18446744073709551615", Some(u64::MAX)),
            ("007", None),
            ("00", None),
            ("+1", None),
            ("-1", None),
            ("", None),
            ("1e3", None),
            ("18446744073709551616", None),
        ] {
            assert_eq!(number(segment), parsed, "{segment}");
        }
    }

    /// a salt-only or label-only authority was v2's local convenience; v3
    /// withdrew it, so neither is an address any more.
    #[test]
    fn a_half_chain_id_is_not_an_authority() {
        for half in ["b5b6ea90", "dognet", "dognet-", "-b5b6ea90", "#b5b6ea90"] {
            assert!(
                sentence(half.parse::<ChainId>()).contains(AUTHORITY),
                "{half}"
            );
        }
    }
}
