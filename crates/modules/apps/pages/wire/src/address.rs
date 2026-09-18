//! pages' half of a `duck://` address.
//!
//! The shared parser ([`duck_address::Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, so pages' answer lives here,
//! beside the wire surface everything else links, and not in the grammar.

use duck_address::{Address, ChainId, Refused};
use sdk::refusal::INVALID_INPUT;

/// pages' path: `duck://<chain>/pages/<page>` or
/// `duck://<chain>/pages/<page>/block/<block>`.
///
/// `page` and `block` are the ids the module keys them by, any name a segment
/// can carry; `block` is a keyword. A block id alone would resolve (block ids
/// are unique module-wide), but a link names the page it opens as well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageAddress {
    pub page: String,
    pub block: Option<String>,
}

const FORM: &str = "`duck://<chain>/pages/<page>` or `duck://<chain>/pages/<page>/block/<block>`";

impl TryFrom<&Address> for PageAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        if address.module != "pages" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A page address is {FORM}, but this one names the module `{}`.",
                    address.module
                ),
            ));
        }
        match address.path.as_slice() {
            [page] => Ok(PageAddress {
                page: page.clone(),
                block: None,
            }),
            [page, keyword, block] if keyword == "block" => Ok(PageAddress {
                page: page.clone(),
                block: Some(block.clone()),
            }),
            _ => Err(Refused::new(
                INVALID_INPUT,
                format!("A page address is {FORM}, and `{address}` is neither."),
            )),
        }
    }
}

impl PageAddress {
    /// the address this page, or this block in it, is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let mut path = vec![self.page.clone()];
        if let Some(block) = &self.block {
            path.extend(["block".to_string(), block.clone()]);
        }
        let address = Address::new(chain, "pages", path)?;
        PageAddress::try_from(&address)?;
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

    #[test]
    fn every_form_round_trips() {
        for (text, typed) in [
            ("duck://dognet-b5b6ea90/pages/p-1", ("p-1", None)),
            (
                "duck://dognet-b5b6ea90/pages/p-1/block/Blk_7",
                ("p-1", Some("Blk_7")),
            ),
            (
                "duck://dognet-b5b6ea90/pages/block/block/block",
                ("block", Some("block")),
            ),
            (
                "duck://dognet-b5b6ea90/pages/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final",
                ("보고서 Final", None),
            ),
        ] {
            let parsed = address(text);
            let page = PageAddress::try_from(&parsed).expect("a page address");
            assert_eq!(
                page,
                PageAddress {
                    page: typed.0.to_string(),
                    block: typed.1.map(str::to_string),
                }
            );
            let printed = page.address(parsed.chain.clone()).expect("prints");
            assert_eq!(printed, parsed);
            assert_eq!(printed.to_string(), text);
        }
    }

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        for (text, rule) in [
            ("duck://dognet-b5b6ea90/chat/p-1", "names the module `chat`"),
            ("duck://dognet-b5b6ea90/pages", "is neither"),
            ("duck://dognet-b5b6ea90/pages/p-1/block", "is neither"),
            ("duck://dognet-b5b6ea90/pages/p-1/b-2", "is neither"),
            ("duck://dognet-b5b6ea90/pages/p-1/blocks/b-2", "is neither"),
            ("duck://dognet-b5b6ea90/pages/p-1/Block/b-2", "is neither"),
            ("duck://dognet-b5b6ea90/pages/p-1/block/b-2/x", "is neither"),
        ] {
            let refused = sentence(PageAddress::try_from(&address(text)));
            assert!(refused.contains(rule), "{text}: {refused}");
        }
    }

    #[test]
    fn an_empty_id_does_not_print() {
        let chain: ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
        for page in [
            PageAddress {
                page: String::new(),
                block: None,
            },
            PageAddress {
                page: "p-1".to_string(),
                block: Some(String::new()),
            },
        ] {
            assert!(sentence(page.address(chain.clone())).contains("none of them empty"));
        }
    }
}
