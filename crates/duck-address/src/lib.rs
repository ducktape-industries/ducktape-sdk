//! the `duck://` address: one grammar for naming a thing on a ducktape
//! network, parsed in one place.
//!
//! ```text
//! duck://<label>-<salt>/<module>/<module-path…>
//! duck://dognet-b5b6ea90/forge/<owner>/<repo>
//! ```
//!
//! THE AUTHORITY IS THE CHAIN ID. The workspace registry keys a network
//! `<label>#<salt>` (core's `mint_chain_id` spells it, the salt being 8 hex
//! digits of the genesis digest), and `#` starts a URL fragment, so an address
//! writes the same pair with `-`. The salt is the match key; the label is
//! display. Whether a label AGREES with the registry is not a question this
//! crate can answer — that needs `~/.ducktape/registry.json` and this crate
//! reads nothing — so `label_mismatch` is core's refusal, not ours. A
//! salt-only or label-only authority is refused: the address carries the chain
//! id whole, exactly as the registry keys it.
//!
//! THE FIRST PATH SEGMENT NAMES A MODULE and everything after it belongs to
//! that module. This parser keeps the tail as segments and interprets none of
//! it: forge's `<owner>/<repo>` rule lives beside forge, in `forge-wire`'s
//! `ForgeRepoAddress`, so a module's name rule has one home and this grammar
//! does not grow a branch per module.
//!
//! Lowercase throughout, and nothing here case-folds: two spellings of one
//! address would be two cache keys, two lock-file lines and two registry
//! lookups.
//!
//! See ducktape#2616 (design note v3) for why the grammar is this.

use refusal_class::INVALID_INPUT;

/// the scheme, spelled once.
const SCHEME: &str = "duck://";

/// how many hex digits the salt is — `mint_chain_id` takes 4 bytes off the
/// genesis digest and writes them as 8 lowercase hex.
const SALT_HEX: usize = 8;

/// the network, as the workspace registry keys it: `<label>#<salt>`.
///
/// `salt` is the match key (4 raw bytes, so a comparison cannot be fooled by a
/// spelling); `label` is what a human reads. Both are public because a
/// consumer resolving an address against the registry needs both halves — this
/// is a pair, not an invariant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainId {
    pub label: String,
    pub salt: [u8; 4],
}

impl ChainId {
    /// the URL spelling of the same pair: `<label>-<salt>`, what a `duck://`
    /// address puts in its authority.
    pub fn authority(&self) -> String {
        format!("{}-{}", self.label, self.salt_hex())
    }

    /// the salt's 8 lowercase hex digits — the spelling the registry, a node's
    /// status and a push certificate all use. ONE home for it, so nothing
    /// re-derives the padding.
    pub fn salt_hex(&self) -> String {
        format!("{:08x}", u32::from_be_bytes(self.salt))
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
    /// `<label>-<salt>` as an address writes it.
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
        let salted = salt.len() == SALT_HEX && salt.bytes().all(|byte| byte.is_ascii_hexdigit());
        if !labelled || !salted {
            return Err(incomplete(text));
        }
        let Ok(salt) = u32::from_str_radix(salt, 16) else {
            return Err(incomplete(text));
        };
        Ok(ChainId {
            label: label.to_string(),
            salt: salt.to_be_bytes(),
        })
    }
}

/// a parsed `duck://` address.
///
/// `path` is everything after the module segment, kept verbatim: what it means
/// is the module's question. `Address::parse` and [`Display`](std::fmt::Display)
/// round-trip — a parsed address prints the string it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub chain: ChainId,
    pub module: String,
    pub path: Vec<String>,
}

impl Address {
    pub fn parse(text: &str) -> Result<Self, Refused> {
        lowercase(text)?;
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
            if !segment
                .bytes()
                .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
            {
                return Err(Refused::new(
                    INVALID_INPUT,
                    format!(
                        "A duck:// path segment carries only [a-z0-9._-], and `{segment}` does not."
                    ),
                ));
            }
            segments.push(segment.to_string());
        }
        let module = segments.remove(0);
        // one module claims a name today. The others (gateway browse, in-app
        // pages) migrate onto this grammar in their own units — ducktape#2616
        // §4 — and each adds its name HERE, so an address for a module nobody
        // has built cannot be read as one for a module that exists.
        if module != "forge" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!("`{module}` names no ducktape module; the module segment is `forge`."),
            ));
        }
        Ok(Address {
            chain,
            module,
            path: segments,
        })
    }
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
            write!(formatter, "/{segment}")?;
        }
        Ok(())
    }
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
/// `new` is public: a module validating its own half of a path (forge-wire's
/// `ForgeRepoAddress`) refuses in the same shape rather than inventing a
/// second error type for the same grammar.
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
                "A duck:// address is lowercase throughout and nothing case-folds it, but `{text}` carries an uppercase letter."
            ),
        )),
        false => Ok(()),
    }
}

fn incomplete(text: &str) -> Refused {
    Refused::new(
        INVALID_INPUT,
        format!(
            "A duck:// authority is the whole chain id `<label>-<salt>` (the registry's `<label>#<salt>`), with `<label>` matching [a-z0-9-] and `<salt>` exactly {SALT_HEX} lowercase hex digits; `{text}` is not one."
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

    const UPPERCASE: &str = "is lowercase throughout";
    const NO_SCHEME: &str = "starts with `duck://`";
    const EXTRA: &str = "carries no credentials, port, query or fragment";
    const EMPTY: &str = "none of them empty";
    const AUTHORITY: &str = "authority is the whole chain id";
    const CHARSET: &str = "path segment carries only [a-z0-9._-]";
    const NO_MODULE: &str = "names no ducktape module";

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
            ("duck://dognet-b5b6ea90/forge/a~b", CHARSET),
            ("duck://dognet-b5b6ea90/gateway/a/b", NO_MODULE),
        ] {
            let sentence = refused(text);
            assert!(sentence.contains(rule), "{text}: {sentence}");
        }
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
