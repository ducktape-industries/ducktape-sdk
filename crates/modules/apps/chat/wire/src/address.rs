//! chat's half of a `duck://` address.
//!
//! The shared parser ([`duck_address::Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, so chat's answer lives here,
//! beside the wire surface everything else links, and not in the grammar.

use duck_address::{Address, ChainId, Refused, number};
use sdk::refusal::INVALID_INPUT;

/// chat's path: `duck://<chain>/chat/<channel>` or
/// `duck://<chain>/chat/<channel>/<seq>` — a channel, or one message in it by
/// the sequence number the module assigned. `channel` is any name a segment
/// can carry; `seq` has one spelling, decimal with no sign and no leading zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageAddress {
    pub channel: String,
    pub seq: Option<u64>,
}

const FORM: &str = "`duck://<chain>/chat/<channel>` or `duck://<chain>/chat/<channel>/<seq>`";

impl TryFrom<&Address> for MessageAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        if address.module != "chat" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A chat address is {FORM}, but this one names the module `{}`.",
                    address.module
                ),
            ));
        }
        match address.path.as_slice() {
            [channel] => Ok(MessageAddress {
                channel: channel.clone(),
                seq: None,
            }),
            [channel, seq] => match number(seq) {
                Some(seq) => Ok(MessageAddress {
                    channel: channel.clone(),
                    seq: Some(seq),
                }),
                None => Err(Refused::new(
                    INVALID_INPUT,
                    format!(
                        "A chat message number is decimal with no sign and no leading zero, within 64 bits, and `{seq}` is not."
                    ),
                )),
            },
            _ => Err(Refused::new(
                INVALID_INPUT,
                format!("A chat address is {FORM}, and `{address}` is neither."),
            )),
        }
    }
}

impl MessageAddress {
    /// the address this channel, or this message in it, is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let mut path = vec![self.channel.clone()];
        path.extend(self.seq.map(|seq| seq.to_string()));
        let address = Address::new(chain, "chat", path)?;
        MessageAddress::try_from(&address)?;
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
        for (text, channel, seq) in [
            ("duck://dognet-b5b6ea90/chat/general", "general", None),
            ("duck://dognet-b5b6ea90/chat/general/0", "general", Some(0)),
            (
                "duck://dognet-b5b6ea90/chat/general/42",
                "general",
                Some(42),
            ),
            (
                "duck://dognet-b5b6ea90/chat/forge%3Aducktape%3A7/18446744073709551615",
                "forge:ducktape:7",
                Some(u64::MAX),
            ),
        ] {
            let parsed = address(text);
            let message = MessageAddress::try_from(&parsed).expect("a chat address");
            assert_eq!(
                message,
                MessageAddress {
                    channel: channel.to_string(),
                    seq,
                }
            );
            let printed = message.address(parsed.chain.clone()).expect("prints");
            assert_eq!(printed, parsed);
            assert_eq!(printed.to_string(), text);
        }
    }

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        for (text, rule) in [
            (
                "duck://dognet-b5b6ea90/pages/general",
                "names the module `pages`",
            ),
            ("duck://dognet-b5b6ea90/chat", "is neither"),
            ("duck://dognet-b5b6ea90/chat/general/1/2", "is neither"),
            ("duck://dognet-b5b6ea90/chat/general/007", "`007` is not"),
            ("duck://dognet-b5b6ea90/chat/general/%2B1", "`+1` is not"),
            ("duck://dognet-b5b6ea90/chat/general/-1", "`-1` is not"),
            ("duck://dognet-b5b6ea90/chat/general/seq", "`seq` is not"),
            (
                "duck://dognet-b5b6ea90/chat/general/18446744073709551616",
                "within 64 bits",
            ),
        ] {
            let refused = sentence(MessageAddress::try_from(&address(text)));
            assert!(refused.contains(rule), "{text}: {refused}");
        }
    }

    #[test]
    fn an_empty_channel_does_not_print() {
        let chain: ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
        let message = MessageAddress {
            channel: String::new(),
            seq: Some(1),
        };
        assert!(sentence(message.address(chain)).contains("none of them empty"));
    }
}
