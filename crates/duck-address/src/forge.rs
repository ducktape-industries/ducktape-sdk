//! forge's half of a `duck://` address.
//!
//! The shared parser ([`Address`]) reads
//! `duck://<chain>/<module>/<module-path…>` and stops at the module segment:
//! what the tail MEANS is the module's question, and this is forge's answer. It
//! ships here, not in `forge-wire`, so a view names a repository or something
//! in one by linking this crate alone; `forge-wire` re-exports it.

use crate::{Address, ChainId, Refused, number};
use refusal_class::INVALID_INPUT;

/// forge's path: `duck://<chain>/forge/<owner>/<repo>`.
///
/// NOTE the namespace this names does not exist in the forge module yet — its
/// repo namespace per network is flat today (one `repo` slug, no `/`). The
/// address carries `<owner>/<repo>` because that is the grammar the owner
/// fixed (ducktape#2616 §3, design note v3); forge learns the namespace in the
/// same wave as the git helper, and until it does the module refuses what this
/// parses. Nothing maps silently in between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeRepoAddress {
    pub owner: String,
    pub repo: String,
}

/// MIRRORED from the forge module's `MAX_REPO_NAME_LEN`
/// (`crates/modules/apps/forge/src/lib.rs` in the ducktape repository, the home
/// of `norm_repo` — the validator the module and noded's git smart-HTTP layer
/// share). Mirrored and not linked: the module lives behind the chain line in
/// another repository, and this crate (re-exported by forge-wire) is what a
/// CLI, a helper and a view link instead of it. A name this admits and the
/// module refuses is a bug in one of the two numbers.
pub const MAX_REPO_NAME_LEN: usize = 64;

impl TryFrom<&Address> for ForgeRepoAddress {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        forge(address, "`duck://<chain>/forge/<owner>/<repo>`")?;
        let [owner, repo] = address.path.as_slice() else {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A forge address is `duck://<chain>/forge/<owner>/<repo>` — two segments after `forge` — and this one carries {}.",
                    address.path.len()
                ),
            ));
        };
        ForgeRepoAddress::named(owner, repo)
    }
}

impl ForgeRepoAddress {
    /// read a forge repository NAME. Forge names a repository
    /// `<owner>/<repo>` and lists it that way, so a caller holding a listed
    /// name gets its address from the name alone and never derives an owner.
    /// A name with no `/` is a repository from before the namespace: it stays
    /// readable in forge and has no address.
    pub fn from_name(name: &str) -> Result<Self, Refused> {
        match name.split_once('/') {
            Some((owner, repo)) if !repo.contains('/') => Self::named(owner, repo),
            _ => Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "A forge repository is named `<owner>/<repo>` with exactly one `/`, and `{name}` is not, so it has no address."
                ),
            )),
        }
    }

    /// the name forge lists this repository under: `<owner>/<repo>`.
    pub fn name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// the address this repository is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let address = Address::new(chain, "forge", vec![self.owner.clone(), self.repo.clone()])?;
        ForgeRepoAddress::try_from(&address)?;
        Ok(address)
    }

    /// the repository half both typed forge addresses share.
    fn named(owner: &str, repo: &str) -> Result<Self, Refused> {
        if repo.ends_with(".git") {
            return Err(Refused::new(
                INVALID_INPUT,
                format!(
                    "Drop the `.git` from `{repo}`: an address names the forge repository, not a directory, and git is happy without it."
                ),
            ));
        }
        Ok(ForgeRepoAddress {
            owner: name("owner", owner)?,
            repo: name("repository", repo)?,
        })
    }
}

/// something inside a forge repository: `duck://<chain>/forge/<owner>/<repo>/`
/// then `<n>`, `<n>/comment/<seq>` or `blob/<rev>/<path…>`.
///
/// A bare `<owner>/<repo>` is not a locator — it is a [`ForgeRepoAddress`],
/// the form git and Cargo read — so a caller holding one tries that first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForgeLocator {
    pub repo: ForgeRepoAddress,
    pub target: ForgeTarget,
}

/// what a [`ForgeLocator`] points at inside its repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForgeTarget {
    /// an issue or pull request — one number space for both: `<n>`.
    Item { number: u64 },
    /// one comment on an item, by its sequence number: `<n>/comment/<seq>`.
    Comment { number: u64, seq: u64 },
    /// a file at a revision: `blob/<rev>/<path…>`, the path at least one
    /// segment. `rev` is a commit id, 40 lowercase hex — the one revision
    /// forge-wire's `ForgeQuery::Blob` reads a file at besides the default head.
    /// forge-wire rules no ref short name, and a branch's may carry a `/`
    /// that no segment can, so a ref is not a `rev` here.
    Blob { rev: String, path: Vec<String> },
}

impl TryFrom<&Address> for ForgeLocator {
    type Error = Refused;

    fn try_from(address: &Address) -> Result<Self, Refused> {
        const FORM: &str = "`duck://<chain>/forge/<owner>/<repo>/` then `<n>`, `<n>/comment/<seq>` or `blob/<rev>/<path…>`";
        forge(address, FORM)?;
        let shape = || {
            Refused::new(
                INVALID_INPUT,
                format!(
                    "A forge locator is {FORM}, and `{address}` is none of them — a bare `<owner>/<repo>` is a repository address, not a locator."
                ),
            )
        };
        let [owner, repo, rest @ ..] = address.path.as_slice() else {
            return Err(shape());
        };
        let repo = ForgeRepoAddress::named(owner, repo)?;
        let target = match rest {
            [item] => ForgeTarget::Item {
                number: counted("item", item)?,
            },
            [item, keyword, seq] if keyword == "comment" => ForgeTarget::Comment {
                number: counted("item", item)?,
                seq: counted("comment", seq)?,
            },
            [keyword, rev, path @ ..] if keyword == "blob" && !path.is_empty() => {
                ForgeTarget::Blob {
                    rev: commit(rev)?,
                    path: path.to_vec(),
                }
            }
            _ => return Err(shape()),
        };
        Ok(ForgeLocator { repo, target })
    }
}

impl ForgeLocator {
    /// the address this locator is at on `chain`.
    pub fn address(&self, chain: ChainId) -> Result<Address, Refused> {
        let mut path = vec![self.repo.owner.clone(), self.repo.repo.clone()];
        match &self.target {
            ForgeTarget::Item { number } => path.push(number.to_string()),
            ForgeTarget::Comment { number, seq } => {
                path.extend([number.to_string(), "comment".to_string(), seq.to_string()])
            }
            ForgeTarget::Blob { rev, path: file } => {
                path.extend(["blob".to_string(), rev.clone()]);
                path.extend(file.iter().cloned());
            }
        }
        let address = Address::new(chain, "forge", path)?;
        ForgeLocator::try_from(&address)?;
        Ok(address)
    }
}

fn forge(address: &Address, form: &str) -> Result<(), Refused> {
    match address.module == "forge" {
        true => Ok(()),
        false => Err(Refused::new(
            INVALID_INPUT,
            format!(
                "A forge address is {form}, but this one names the module `{}`.",
                address.module
            ),
        )),
    }
}

fn counted(part: &str, value: &str) -> Result<u64, Refused> {
    number(value).ok_or_else(|| {
        Refused::new(
            INVALID_INPUT,
            format!(
                "A forge {part} number is decimal with no sign and no leading zero, within 64 bits, and `{value}` is not."
            ),
        )
    })
}

fn commit(rev: &str) -> Result<String, Refused> {
    match rev.len() == 40
        && rev
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        true => Ok(rev.to_string()),
        false => Err(Refused::new(
            INVALID_INPUT,
            format!(
                "A forge blob revision is a commit id, 40 lowercase hex digits, and `{rev}` is not."
            ),
        )),
    }
}

/// forge's name rule, as the module admits it: `[a-z0-9._-]`, never starting
/// with `.` (that collides with `.`/`..` as a path segment and with forge's own
/// dot-prefixed state files), 1..=[`MAX_REPO_NAME_LEN`] bytes. One token for
/// every way a name can fail it, because a caller does the same thing about all
/// of them: fix the name.
fn name(part: &str, value: &str) -> Result<String, Refused> {
    let refuse = |why: &str| {
        Err(Refused::new(
            INVALID_INPUT,
            format!("A forge {part} name {why}, and `{value}` does not."),
        ))
    };
    if value.is_empty() || value.len() > MAX_REPO_NAME_LEN {
        return refuse(&format!("is 1 to {MAX_REPO_NAME_LEN} bytes"));
    }
    if value.starts_with('.') {
        return refuse("never starts with `.`");
    }
    if !value
        .bytes()
        .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'))
    {
        return refuse("carries only [a-z0-9._-]");
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str) -> Address {
        Address::parse(text).expect("the shared grammar parses this")
    }

    /// every forge refusal is one class (fix the address), so the sentence
    /// is what tells the rules apart.
    fn refused(text: &str) -> String {
        sentence(ForgeRepoAddress::try_from(&address(text)))
    }

    fn sentence<T: std::fmt::Debug>(parsed: Result<T, Refused>) -> String {
        let refused = parsed.expect_err("refused");
        assert_eq!(refused.reason, INVALID_INPUT);
        refused.sentence
    }

    #[test]
    fn reads_owner_and_repo() {
        let parsed =
            ForgeRepoAddress::try_from(&address("duck://dognet-b5b6ea90/forge/alice/ducktape-sdk"))
                .expect("accepted");
        assert_eq!(parsed.owner, "alice");
        assert_eq!(parsed.repo, "ducktape-sdk");
    }

    #[test]
    fn the_path_is_exactly_two_segments() {
        for text in [
            "duck://dognet-b5b6ea90/forge",
            "duck://dognet-b5b6ea90/forge/alice",
            "duck://dognet-b5b6ea90/forge/alice/my-crate/src",
        ] {
            assert!(
                refused(text).contains("two segments after `forge`"),
                "{text}"
            );
        }
    }

    #[test]
    fn a_dot_git_suffix_is_refused() {
        assert!(
            refused("duck://dognet-b5b6ea90/forge/alice/my-crate.git")
                .starts_with("Drop the `.git`")
        );
    }

    #[test]
    fn a_name_forge_would_refuse_is_refused_here() {
        assert!(
            refused("duck://dognet-b5b6ea90/forge/alice/.tracker.bin")
                .starts_with("A forge repository name never starts with `.`")
        );
        assert!(
            refused("duck://dognet-b5b6ea90/forge/.alice/repo")
                .starts_with("A forge owner name never starts with `.`")
        );
        let long = "a".repeat(MAX_REPO_NAME_LEN + 1);
        assert!(
            refused(&format!("duck://dognet-b5b6ea90/forge/alice/{long}"))
                .starts_with("A forge repository name is 1 to")
        );
        assert!(
            ForgeRepoAddress::try_from(&address(&format!(
                "duck://dognet-b5b6ea90/forge/alice/{}",
                "a".repeat(MAX_REPO_NAME_LEN)
            )))
            .is_ok()
        );
    }

    /// forge narrows the shared grammar: a path segment may carry any name
    /// (`~`, uppercase, a percent-encoded space), and a forge name only
    /// `[a-z0-9._-]`. So a name forge refuses arrives through `Address::parse`
    /// as readily as through a hand-built `Address`, and this check is forge's
    /// own — the module's validator is the one that matters, and this mirrors
    /// it whole rather than leaning on the parser upstream.
    #[test]
    fn the_name_rule_does_not_lean_on_the_parser() {
        let built = Address {
            chain: "dognet-b5b6ea90".parse().expect("a chain id parses"),
            module: "forge".to_string(),
            path: vec!["alice".to_string(), "my~crate".to_string()],
        };
        assert!(
            sentence(ForgeRepoAddress::try_from(&built))
                .starts_with("A forge repository name carries only [a-z0-9._-]")
        );
    }

    const REV: &str = "0123456789abcdef0123456789abcdef01234567";

    /// every form prints from its typed parts to the string it parsed from.
    #[test]
    fn every_form_round_trips() {
        let repo = "duck://dognet-b5b6ea90/forge/alice/ducktape-sdk";
        let parsed = address(repo);
        let typed = ForgeRepoAddress::try_from(&parsed).expect("a repository");
        let printed = typed.address(parsed.chain.clone()).expect("prints");
        assert_eq!(printed, parsed);
        assert_eq!(printed.to_string(), repo);
        for text in [
            format!("{repo}/42"),
            format!("{repo}/0/comment/7"),
            format!("{repo}/blob/{REV}/src/lib.rs"),
            format!("{repo}/blob/{REV}/docs/%EB%B3%B4%EA%B3%A0%EC%84%9C%20Final.md"),
        ] {
            let parsed = address(&text);
            let typed = ForgeLocator::try_from(&parsed).expect("a locator");
            let printed = typed.address(parsed.chain.clone()).expect("prints");
            assert_eq!(printed, parsed);
            assert_eq!(printed.to_string(), text);
        }
        assert_eq!(
            ForgeLocator::try_from(&address(&format!("{repo}/12/comment/3")))
                .expect("a locator")
                .target,
            ForgeTarget::Comment { number: 12, seq: 3 }
        );
    }

    #[test]
    fn every_locator_refusal_names_the_rule_it_broke() {
        let repo = "duck://dognet-b5b6ea90/forge/alice/ducktape-sdk";
        for (text, rule) in [
            (
                "duck://dognet-b5b6ea90/pages/alice/ducktape-sdk/1".to_string(),
                "names the module `pages`",
            ),
            (repo.to_string(), "is a repository address, not a locator"),
            (
                "duck://dognet-b5b6ea90/forge".to_string(),
                "is none of them",
            ),
            (format!("{repo}/1/comment"), "is none of them"),
            (format!("{repo}/1/2"), "is none of them"),
            (format!("{repo}/1/comments/2"), "is none of them"),
            (format!("{repo}/tree/{REV}/src"), "is none of them"),
            (format!("{repo}/blob/{REV}"), "is none of them"),
            (format!("{repo}/007"), "item number is decimal"),
            (format!("{repo}/%2B1"), "item number is decimal"),
            (format!("{repo}/1/comment/01"), "comment number is decimal"),
            (
                format!("{repo}/blob/main/a"),
                "40 lowercase hex digits, and `main`",
            ),
            (
                format!("{repo}/blob/{}/a", REV.to_uppercase()),
                "40 lowercase hex",
            ),
            (format!("{repo}/blob/{}/a", &REV[1..]), "40 lowercase hex"),
            (
                "duck://dognet-b5b6ea90/forge/alice/r.git/1".to_string(),
                "Drop the `.git`",
            ),
        ] {
            let refused = sentence(ForgeLocator::try_from(&address(&text)));
            assert!(refused.contains(rule), "{text}: {refused}");
        }
    }

    /// a typed value built by hand prints only if it would parse back to itself.
    #[test]
    fn a_hand_built_locator_prints_only_if_it_parses_back() {
        let locator = ForgeLocator {
            repo: ForgeRepoAddress {
                owner: "alice".to_string(),
                repo: "my~crate".to_string(),
            },
            target: ForgeTarget::Item { number: 1 },
        };
        let chain: ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
        assert!(
            sentence(locator.address(chain.clone()))
                .starts_with("A forge repository name carries only [a-z0-9._-]")
        );
        let blob = ForgeLocator {
            target: ForgeTarget::Blob {
                rev: REV.to_string(),
                path: vec!["a/b".to_string()],
            },
            ..locator
        };
        assert!(sentence(blob.address(chain)).contains("no `/` and no NUL"));
    }

    #[test]
    fn another_module_is_not_a_forge_repo() {
        let built = Address {
            chain: "dognet-b5b6ea90".parse().expect("a chain id parses"),
            module: "gateway".to_string(),
            path: vec!["alice".to_string(), "page".to_string()],
        };
        assert!(
            sentence(ForgeRepoAddress::try_from(&built)).contains("names the module `gateway`")
        );
    }

    #[test]
    fn a_listed_name_is_its_address_and_a_flat_name_has_none() {
        let repo = ForgeRepoAddress::from_name("ducktape-industries/ducktape-sdk").expect("named");
        assert_eq!(repo.owner, "ducktape-industries");
        assert_eq!(repo.repo, "ducktape-sdk");
        assert_eq!(repo.name(), "ducktape-industries/ducktape-sdk");
        for flat in ["ducktape-sdk", "a/b/c", "", "/repo", "owner/"] {
            let refused = ForgeRepoAddress::from_name(flat).expect_err(flat);
            assert_eq!(refused.reason, INVALID_INPUT);
        }
    }
}
