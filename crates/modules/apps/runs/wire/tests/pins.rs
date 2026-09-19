//! What this crate copies from a sibling it cannot afford to name.
//!
//! `runs-wire` exists so a view or the daemon can link the runs format without
//! linking runs. `pages` would defeat that for one number: it depends on `files`
//! with its `native` feature, so naming it here would put duckfs-disk and
//! `std::fs` into every consumer. So the limit is copied — and a copy without a
//! pin is a lie with a delay on it, which is what this file is for.
//!
//! `pages` is a DEV-dependency: `cargo build` never links one, so none of that
//! graph reaches a component. Only this test binary sees it.

#[test]
fn pages_title_limit_is_the_one_pages_enforces() {
    assert_eq!(
        runs_wire::MAX_PAGE_TITLE_LEN,
        pages::MAX_PAGE_TITLE_LEN,
        "the catalog publishes this limit in the `pages.post` input schema, so a \
         program is told what pages will accept before it writes. If pages moved \
         its limit, update runs-wire's copy — do not relax this test.",
    );
}

/// `duck-address` cannot name this crate, so `RunAddress` copies the shape of
/// a dispatch id (64 lowercase hex) instead of calling `dispatch_id_for`. This
/// pins the copy: every id this crate mints is a run address.
#[test]
fn a_dispatch_id_is_a_run_address() {
    let chain: duck_address::ChainId = "dognet-b5b6ea90".parse().expect("a chain id parses");
    let run = runs_wire::RunAddress {
        digest: runs_wire::dispatch_id_for("chat\u{1f}general\u{1f}7\u{1f}bot"),
    };
    let printed = run.address(chain).expect("prints");
    assert_eq!(runs_wire::RunAddress::try_from(&printed), Ok(run));
}
