//! runs' half of a `duck://` address.
//!
//! The shared parser ([`duck_address::Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, so runs' answer lives here,
//! beside the wire surface everything else links, and not in the grammar.

use duck_address::{Address, ChainId, Refused};
use sdk::refusal::INVALID_INPUT;

/// runs' path: `duck://<chain>/runs/<digest>`, exactly one segment.
///
/// `digest` is the run's dispatch id — [`crate::dispatch_id_for`] of its run
/// id, 64 lowercase hex — the id that addresses a run everywhere outside the
/// module ([`crate::RunsViewQuery::Run`]). The run id itself carries reserved
/// separators and is never a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunAddress {
    pub digest: String,
}

impl TryFrom<&Address> for RunAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        if address.module != "runs" {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A run address is `duck://<chain>/runs/<digest>`, but this one names the module `{}`.",
                    address.module
                ),
            ));
        }
        let [digest] = address.path.as_slice() else {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A run address is `duck://<chain>/runs/<digest>` — one segment after `runs` — and this one carries {}.",
                    address.path.len()
                ),
            ));
        };
        let hex = digest.len() == 64
            && digest
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if !hex {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A run digest is the run's dispatch id, 64 lowercase hex digits, and `{digest}` is not."
                ),
            ));
        }
        Ok(RunAddress {
            digest: digest.clone(),
        })
    }
}

impl RunAddress {
    /// the address this run is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let address = Address::new(chain, "runs", vec![self.digest.clone()])?;
        RunAddress::try_from(&address)?;
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

    fn digest() -> String {
        crate::dispatch_id_for("chat\u{1f}general\u{1f}7\u{1f}bot")
    }

    #[test]
    fn a_dispatch_id_round_trips() {
        let text = format!("duck://dognet-b5b6ea90/runs/{}", digest());
        let parsed = address(&text);
        let run = RunAddress::try_from(&parsed).expect("a run address");
        assert_eq!(run.digest, digest());
        let printed = run.address(parsed.chain.clone()).expect("prints");
        assert_eq!(printed, parsed);
        assert_eq!(printed.to_string(), text);
    }

    #[test]
    fn every_refusal_names_the_rule_it_broke() {
        let digest = digest();
        for (text, rule) in [
            (
                format!("duck://dognet-b5b6ea90/chat/{digest}"),
                "names the module `chat`",
            ),
            ("duck://dognet-b5b6ea90/runs".to_string(), "carries 0"),
            (
                format!("duck://dognet-b5b6ea90/runs/{digest}/journal"),
                "carries 2",
            ),
            (
                format!("duck://dognet-b5b6ea90/runs/{}", &digest[1..]),
                "64 lowercase hex",
            ),
            (
                format!("duck://dognet-b5b6ea90/runs/{digest}0"),
                "64 lowercase hex",
            ),
            (
                format!("duck://dognet-b5b6ea90/runs/{}", digest.to_uppercase()),
                "64 lowercase hex",
            ),
            (
                format!("duck://dognet-b5b6ea90/runs/{}", "g".repeat(64)),
                "64 lowercase hex",
            ),
        ] {
            let refused = sentence(RunAddress::try_from(&address(&text)));
            assert!(refused.contains(rule), "{text}: {refused}");
        }
    }

    #[test]
    fn a_hand_built_run_prints_only_if_it_parses_back() {
        let chain: ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
        for digest in ["", "run-1"] {
            let run = RunAddress {
                digest: digest.to_string(),
            };
            assert!(run.address(chain.clone()).is_err(), "{digest:?}");
        }
    }
}
