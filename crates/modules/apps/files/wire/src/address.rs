//! files' half of a `duck://` address.
//!
//! The shared parser ([`duck_address::Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, so files' answer lives here,
//! beside the wire surface everything else links, and not in the grammar.

use duck_address::{Address, ChainId, Refused};
use refusal_class::INVALID_INPUT;

/// files' path: `duck://<chain>/files/<segment>/<segment…>`, the absolute
/// duckfs path one segment per segment, at least one.
///
/// A segment is any name duckfs holds — `보고서 Final.pdf`, `Notes` — and the
/// path obeys duckfs's own rule, [`duckfs_core::paths::canonical`] (NFC, the
/// byte and depth caps), so an address names a path the module can hold. The
/// first segment is not narrowed to `shared` or `home`: duckfs rules that only
/// for a member's WRITE ([`duckfs_core::paths::check_authority`]), and system
/// writes anywhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileAddress {
    pub path: Vec<String>,
}

impl TryFrom<&Address> for FileAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        if address.module != "files" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A file address is `duck://<chain>/files/<path…>`, but this one names the module `{}`.",
                    address.module
                ),
            ));
        }
        if address.path.is_empty() {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A file address is `duck://<chain>/files/<path…>` with at least one segment after `files`, and `{address}` has none."
                ),
            ));
        }
        let absolute = format!("/{}", address.path.join("/"));
        if let Err(why) = duckfs_core::paths::canonical(&absolute) {
            return Err(Refused::new(
                INVALID_INPUT,
                format!("A file address names a duckfs path, and `{absolute}` is not one: {why}."),
            ));
        }
        Ok(FileAddress {
            path: address.path.clone(),
        })
    }
}

impl FileAddress {
    /// the address this file is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let address = Address::new(chain, "files", self.path.clone())?;
        FileAddress::try_from(&address)?;
        Ok(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Address {
        Address::parse(text).expect("the shared grammar parses this")
    }

    fn sentence<T: std::fmt::Debug>(parsed: Result<T, Refused>) -> String {
        let refused = parsed.expect_err("refused");
        assert_eq!(refused.reason, INVALID_INPUT);
        refused.sentence
    }

    /// a Korean name, a space and uppercase survive the whole string form.
    #[test]
    fn every_path_round_trips() {
        for (text, path) in [
            (
                "duck://dognet-b5b6ea90/files/shared/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.pdf",
                &["shared", "보고서 Final.pdf"][..],
            ),
            (
                "duck://dognet-b5b6ea90/files/home/acct%3A7/Notes/Draft%201.md",
                &["home", "acct:7", "Notes", "Draft 1.md"],
            ),
            ("duck://dognet-b5b6ea90/files/shared", &["shared"]),
            ("duck://dognet-b5b6ea90/files/sys/state", &["sys", "state"]),
        ] {
            let parsed = address(text);
            let file = FileAddress::try_from(&parsed).expect("a file address");
            assert_eq!(file.path, path);
            let printed = file.address(parsed.chain.clone()).expect("prints");
            assert_eq!(printed, parsed);
            assert_eq!(printed.to_string(), text);
        }
    }

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        let long = "a".repeat(256);
        for (text, rule) in [
            (
                "duck://dognet-b5b6ea90/pages/shared/a".to_string(),
                "names the module `pages`",
            ),
            ("duck://dognet-b5b6ea90/files".to_string(), "has none"),
            // `é` decomposed: a name duckfs would never hold
            (
                "duck://dognet-b5b6ea90/files/shared/e%CC%81".to_string(),
                "not NFC-normalized",
            ),
            (
                format!("duck://dognet-b5b6ea90/files/shared/{long}"),
                "byte limit",
            ),
        ] {
            let refused = sentence(FileAddress::try_from(&address(&text)));
            assert!(refused.contains(rule), "{text}: {refused}");
        }
    }

    #[test]
    fn a_hand_built_path_prints_only_if_it_parses_back() {
        let chain: ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
        for path in [
            vec![],
            vec!["shared", ".."],
            vec!["shared/a"],
            vec!["shared", ""],
        ] {
            let file = FileAddress {
                path: path.iter().map(|segment| segment.to_string()).collect(),
            };
            assert!(file.address(chain.clone()).is_err(), "{path:?}");
        }
    }
}
