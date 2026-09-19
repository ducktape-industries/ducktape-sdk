//! identity's half of a `duck://` address.
//!
//! The shared parser ([`Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, and this is identity's answer.
//! It ships here, not in `identity-wire`, so a view names an account by linking
//! this crate alone — identity-wire is the signing graph a wasm32 component
//! cannot build; `identity-wire` re-exports it.

use crate::{Address, ChainId, Refused, number};
use refusal_class::INVALID_INPUT;

/// identity's path: `duck://<chain>/identity/<account>` — one account by the
/// number the module assigned it, decimal with no sign and no leading zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountAddress {
    pub account: u64,
}

const FORM: &str = "`duck://<chain>/identity/<account>`";

impl TryFrom<&Address> for AccountAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        if address.module != "identity" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "An account address is {FORM}, but this one names the module `{}`.",
                    address.module
                ),
            ));
        }
        let [account] = address.path.as_slice() else {
            return Err(Refused::new(
                INVALID_INPUT,
                format!("An account address is {FORM}, and `{address}` is not one."),
            ));
        };
        match number(account) {
            Some(account) => Ok(AccountAddress { account }),
            None => Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "An account number is decimal with no sign and no leading zero, within 64 bits, and `{account}` is not."
                ),
            )),
        }
    }
}

impl AccountAddress {
    /// the address this account is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        Address::new(chain, "identity", vec![self.account.to_string()])
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
    fn every_account_round_trips() {
        for (text, account) in [
            ("duck://dognet-b5b6ea90/identity/0", 0),
            ("duck://dognet-b5b6ea90/identity/7", 7),
            (
                "duck://dognet-b5b6ea90/identity/18446744073709551615",
                u64::MAX,
            ),
        ] {
            let parsed = address(text);
            let typed = AccountAddress::try_from(&parsed).expect("an account address");
            assert_eq!(typed, AccountAddress { account });
            let printed = typed.address(parsed.chain.clone()).expect("prints");
            assert_eq!(printed, parsed);
            assert_eq!(printed.to_string(), text);
        }
    }

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        // the shared grammar refuses an empty segment before a tail reads it,
        // and a struct literal checks nothing, so the tail refuses it too.
        let empty = Address {
            path: vec![String::new()],
            ..address("duck://dognet-b5b6ea90/identity/7")
        };
        for (parsed, rule) in [
            (
                address("duck://dognet-b5b6ea90/chat/7"),
                "names the module `chat`",
            ),
            (address("duck://dognet-b5b6ea90/identity"), "is not one"),
            (address("duck://dognet-b5b6ea90/identity/7/8"), "is not one"),
            (
                address("duck://dognet-b5b6ea90/identity/007"),
                "`007` is not",
            ),
            (
                address("duck://dognet-b5b6ea90/identity/%2B1"),
                "`+1` is not",
            ),
            (address("duck://dognet-b5b6ea90/identity/-1"), "`-1` is not"),
            (
                address("duck://dognet-b5b6ea90/identity/zoe"),
                "`zoe` is not",
            ),
            (
                address("duck://dognet-b5b6ea90/identity/18446744073709551616"),
                "within 64 bits",
            ),
            (empty, "`` is not"),
        ] {
            let refused = sentence(AccountAddress::try_from(&parsed));
            assert!(refused.contains(rule), "{parsed:?}: {refused}");
        }
    }
}
