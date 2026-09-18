//! `object-wasm` — the object-plane + odb-backing reference guest: every op
//! exercises the `object-*` / `state-*` host surface, so the kernel tests can
//! prove the object plumbing (same-dispatch put→stat/get overlay hits, absent-id
//! `None` resolution, the object-read budget, staged-put discard on abort) AND
//! the `StateBacking::Odb` seam (cross-dispatch overlay visibility via 'P', the
//! refs state lane via 'r'). It calls the raw host imports directly (the
//! `GuestOdb` ObjectStore wrapper in `ducktape-module-sdk` is proven separately by
//! compile).
//!
//! ops: 'p' put+same-dispatch-check · 'a' assert-absent · 'P' assert-present ·
//! 'b' budget-probe · 'r' stage-refs-image · 'c' assert-genesis-config —
//! decoded by hand rather than through `sdk::genesis_config` / the module SDK
//! (this guest is deliberately standalone, calling the raw host imports the
//! `GuestOdb` wrapper and `load_config` are proven to sit over), so it proves
//! the odb `__config` seam from a plain reader's point of view too.

wit_bindgen::generate!({
    world: "module",
    path: "../../module-sdk/wit",
});

use ducktape::module::host;

struct Component;

/// this guest's refusal: its own snake_case token for the failure class, then
/// the sentence it refused with. The world carries the pair as ONE string, in
/// the framing `sdk::refusal` defines — spelled out here because a fixture
/// guest links wit-bindgen and nothing else.
fn refused(reason: &str, sentence: impl AsRef<str>) -> host::Error {
    host::Error::Rejected(format!("{reason}: {}", sentence.as_ref()))
}

/// the tagged body a put stores under `id`: `kind ‖ body`.
fn tagged(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut t = Vec::with_capacity(1 + body.len());
    t.push(kind);
    t.extend_from_slice(body);
    t
}

/// the [`sdk::genesis_config::encode_config`] frame, decoded by hand: a
/// `u64-le` count, then per parameter a length-prefixed key and a
/// length-prefixed value. returns the value for `key`, or `None` if the
/// frame is short, malformed, or the key is absent — this guest is a probe,
/// not the consensus decoder, so it fails closed rather than panicking.
fn find_config_value<'a>(bytes: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let (count_bytes, mut rest) = bytes.split_at_checked(8)?;
    let count = u64::from_le_bytes(count_bytes.try_into().ok()?);
    for _ in 0..count {
        let (klen_bytes, after_klen) = rest.split_at_checked(8)?;
        let klen = u64::from_le_bytes(klen_bytes.try_into().ok()?) as usize;
        let (k, after_k) = after_klen.split_at_checked(klen)?;
        let (vlen_bytes, after_vlen) = after_k.split_at_checked(8)?;
        let vlen = u64::from_le_bytes(vlen_bytes.try_into().ok()?) as usize;
        let (v, after_v) = after_vlen.split_at_checked(vlen)?;
        if k == key {
            return Some(v);
        }
        rest = after_v;
    }
    None
}

/// a distinct 32-byte id for budget-probe read `i` (never put, so absent).
fn distinct_id(i: u64) -> Vec<u8> {
    let mut id = vec![0u8; 32];
    id[..8].copy_from_slice(&i.to_le_bytes());
    id
}

impl Guest for Component {
    fn initialize(_params: Vec<u8>) -> Result<(), host::Error> {
        Ok(())
    }

    fn finalize_block() -> Result<(), host::Error> {
        Ok(())
    }

    /// the object plane is the odb backing's: this guest declares odb, so the
    /// host wraps it over an object store (the kernel tests' mock) and never
    /// over a plain map.
    fn pending_items() -> Result<Vec<host::PendingItem>, host::Error> {
        Ok(Vec::new())
    }

    fn acknowledge(_ack: host::Ack) -> Result<(), host::Error> {
        Err(refused("unsupported", "This module has no outbound queue."))
    }

    fn shape() -> host::ModuleShape {
        host::ModuleShape {
            backing: host::Backing::Odb,
            config: vec!["chain_id".into()],
            committed_queries: true,
        }
    }

    fn execute(payload: Vec<u8>) -> Result<(), host::Error> {
        match payload.split_first() {
            // 'p' kind id(32) body.. — put, check the host's id is the one the
            // caller computed (sha256(kind ‖ body)), then stat AND get it IN THE
            // SAME DISPATCH: both must answer from the staged overlay (no pause,
            // no backing). rejects if anything disagrees with the put.
            Some((b'p', rest)) => {
                let (&kind, rest) = rest
                    .split_first()
                    .ok_or_else(|| refused("malformed_op", "put needs a kind byte"))?;
                if rest.len() < 32 {
                    return Err(refused("malformed_op", "put needs a 32-byte expected id"));
                }
                let (expected, body) = rest.split_at(32);
                let id = host::object_put(kind, body);
                if id != expected {
                    return Err(refused(
                        "object_plane_mismatch",
                        "host id differs from sha256(kind ‖ body)",
                    ));
                }
                let want = tagged(kind, body);

                let stat = host::object_stat(&id);
                if stat != Some((kind, body.len() as u64)) {
                    return Err(refused(
                        "object_plane_mismatch",
                        "same-dispatch stat missed the put",
                    ));
                }
                let got = host::object_get(&id);
                if got.as_deref() != Some(want.as_slice()) {
                    return Err(refused(
                        "object_plane_mismatch",
                        "same-dispatch get missed the put",
                    ));
                }
                Ok(())
            }
            // 'a' id(32) — assert the id is ABSENT: get/stat/has all empty. a
            // trap loop (instead of a real None) would blow the budget or hang
            // rather than reach this Ok.
            Some((b'a', id)) => {
                if id.len() != 32 {
                    return Err(refused("malformed_op", "absent probe needs a 32-byte id"));
                }
                if host::object_stat(id).is_some() {
                    return Err(refused(
                        "object_plane_mismatch",
                        "expected-absent id had a stat",
                    ));
                }
                if host::object_get(id).is_some() {
                    return Err(refused(
                        "object_plane_mismatch",
                        "expected-absent id had a body",
                    ));
                }
                Ok(())
            }
            // 'P' id(32) — assert the id is PRESENT (stat AND get both Some):
            // the cross-dispatch overlay hit and the post-publish backing hit
            // are the same op — the mirror of 'a', for the odb-backing proof.
            Some((b'P', id)) => {
                if id.len() != 32 {
                    return Err(refused("malformed_op", "present probe needs a 32-byte id"));
                }
                if host::object_stat(id).is_none() {
                    return Err(refused(
                        "object_plane_mismatch",
                        "expected-present id had no stat",
                    ));
                }
                if host::object_get(id).is_none() {
                    return Err(refused(
                        "object_plane_mismatch",
                        "expected-present id had no body",
                    ));
                }
                Ok(())
            }
            // 'r' bytes.. — stage a new refs image under the reserved state-lane
            // key (the odb backing's single-value refs lane). the host adopts it
            // at commit; `root()` becomes sha256(bytes).
            Some((b'r', bytes)) => {
                host::state_set(b"__state", bytes);
                Ok(())
            }
            // 'b' + le-u64 count — perform `count` DISTINCT object stats: the
            // object-read budget probe. every id is absent, so each read is a
            // fresh memo entry against the budget.
            Some((b'b', rest)) => {
                let n = match rest.len() {
                    8 => {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(rest);
                        u64::from_le_bytes(b)
                    }
                    _ => return Err(refused("malformed_op", "count must be 8 bytes")),
                };
                for i in 0..n {
                    let _ = host::object_stat(&distinct_id(i));
                }
                Ok(())
            }
            // 'c' bytes.. — assert the host-installed `__config` record
            // carries key "chain_id" with exactly `bytes` as its value. reads
            // the same reserved key [`sdk::genesis_config::CONFIG_KEY`] names
            // (`__config`) through the plain `state-get` import and decodes
            // the frame by hand (count, then length-prefixed key/value pairs)
            // — the odb twin of `ducktape_module_sdk::load_config`.
            Some((b'c', want)) => {
                let config = host::state_get(b"__config")
                    .ok_or_else(|| refused("genesis_config", "__config absent"))?;
                let got = find_config_value(&config, b"chain_id")
                    .ok_or_else(|| refused("genesis_config", "chain_id absent from __config"))?;
                if got != want {
                    return Err(refused("genesis_config", "chain_id value mismatch"));
                }
                Ok(())
            }
            _ => Err(refused("unknown_op", "unknown op")),
        }
    }

    /// Queries project committed refs and committed object bodies.
    fn query(req: Vec<u8>) -> Result<Vec<u8>, host::Error> {
        let Some((&tag, rest)) = req.split_first() else { return Err(host::Error::Unsupported); };
        match tag {
            b'r' => {
                let refs = host::state_get(b"__state").unwrap_or_default();
                #[cfg(feature = "replacement")]
                { return Ok([b"updated:".as_slice(), &refs].concat()); }
                #[cfg(not(feature = "replacement"))]
                { Ok(refs) }
            },
            b'o' => Ok(host::object_get(rest).unwrap_or_default()),
            b'g' => host::git_object_read("default", rest, 1024).map(|object| object.raw),
            _ => Err(host::Error::Unsupported),
        }
    }

}

export!(Component);
