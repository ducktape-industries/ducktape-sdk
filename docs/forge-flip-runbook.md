# Flip runbook: ducktape-sdk's git dependencies from GitHub to Forge

Status: **written, not executed.** It runs only after core #2675 lands (the
node's git endpoint answers want-by-SHA) and the `git-remote-duck` helper and
`ducktape forge setup` exist (core #2616, design note v4). ducktape-sdk is the
first repository in the cut-over order because it has exactly one git
dependency.

The rule this runbook keeps (owner direction 6): GitHub stays. `duck://` is a
second spelling of the same commits, the flip is per repository, opt-in, and
reverted by editing the same lines back. `Cargo.lock` pins the commit, so the
flip changes WHERE a commit is fetched from and never WHICH commit is built.

Placeholders: `<chain>` is the network's chain id spelled for a URL
(`<label>-<salt>`, e.g. `dognet-b5b6ea90`); `<node>` is the node's HTTP base.
`REV=76ba1867bc0ba9c09bb8387149c07844f1d2395c` is the fluent31 commit dev pins
today; use whatever `Cargo.toml` pins on the day.

## 0. Preconditions (check, do not assume)

1. core #2675 is merged and the node you resolve through runs it:
   `git fetch http://<node>/<forge git route>/ducktape-industries/fluent31 $REV`
   succeeds for this non-tip commit.
2. `ducktape forge setup --cargo` has run on this machine: `git-remote-duck` is
   on `PATH`, the workspace registry holds `<chain>`, and the user's Cargo
   config has `net.git-fetch-with-cli = true`.
3. Cargo accepts the scheme: in a scratch crate,
   `cargo metadata` with `git = "duck://<chain>/forge/ducktape-industries/fluent31", rev = "$REV"`
   resolves. If Cargo refuses an unknown scheme, STOP: that is a design
   finding for #2616, not something to work around here.
4. The mirror holds the pinned commit (section 1 has run at least once):
   `git ls-remote duck://<chain>/forge/ducktape-industries/fluent31` lists
   `refs/heads/main`, and `git fetch duck://… $REV` succeeds.

## 1. The mirror (node host, no CI)

Forge mirrors `ducktape-industries/fluent31` (and, for section 4,
`ducktape-industries/ducktape-sdk`), branches `main` and `dev` where they
exist. It reuses core's `ops/forge-import.py`, which is already incremental,
fast-forward-only and compare-and-swap on the previous head.

`/usr/local/lib/ducktape/forge-mirror.sh`:

```sh
#!/bin/sh
# usage: forge-mirror.sh <node-url> <token-file> <owner>/<repo> <branch>...
set -eu
node=$1 token=$2 repo=$3; shift 3
dir=/var/lib/ducktape/mirror/$repo.git
[ -d "$dir" ] || git clone --quiet --mirror "https://github.com/$repo" "$dir"
git -C "$dir" fetch --quiet --prune origin
for branch in "$@"; do
  github=$(git -C "$dir" rev-parse --verify "refs/heads/$branch")
  forge=$(ops/forge-import.py head --node-url "$node" --repo "$repo" --branch "$branch")
  [ "$github" = "$forge" ] && continue
  # drift check: Forge must be an ancestor of GitHub, or nobody syncs anything.
  if [ -n "$forge" ] && ! git -C "$dir" merge-base --is-ancestor "$forge" "$github"; then
    echo "DRIFT $repo $branch: forge $forge is not an ancestor of github $github" >&2
    exit 3
  fi
  (cd "$dir" && ops/forge-import.py push --node-url "$node" --token-file "$token" \
     --repo "$repo" --branch "$branch" --tip "$github")
done
```

(`ops/forge-import.py` is addressed by its path in the core checkout on the
node host.) Exit 3 is the drift signal: a GitHub force-push, or a direct push
to the mirror. Nobody force-syncs a mirror; a person reconciles, and the
forge write-authority check (`unauthorized`) is what keeps direct pushes out.

`/etc/systemd/system/forge-mirror@.service`:

```ini
[Unit]
Description=Mirror github.com/%I into Forge
[Service]
Type=oneshot
User=ducktape
ExecStart=/usr/local/lib/ducktape/forge-mirror.sh http://127.0.0.1:<port> /var/lib/ducktape/admin.token %I main dev
```

`/etc/systemd/system/forge-mirror@.timer`:

```ini
[Unit]
Description=Mirror github.com/%I into Forge every 5 minutes
[Timer]
OnBootSec=2min
OnUnitActiveSec=5min
[Install]
WantedBy=timers.target
```

Enable with the instance name systemd-escaped:
`systemctl enable --now "forge-mirror@$(systemd-escape ducktape-industries/fluent31).timer"`.
Mirror lag is bounded by the timer period. A consumer that pins a commit the
mirror does not hold yet gets "not reachable from any ref" from the node: a
visible failure, never a different object. A failed unit (`systemctl
--failed`, exit 3) is the drift alarm.

## 2. The flip (one PR, three files)

1. `Cargo.toml` line `fluent-guest = { git = … }`:
   `https://github.com/ducktape-industries/fluent31` becomes
   `duck://<chain>/forge/ducktape-industries/fluent31`. `rev` does not move.
2. `Cargo.lock`, the `source =` line of `fluent-guest` and of
   `fluent-guest-macros`:
   `git+https://github.com/ducktape-industries/fluent31?rev=$REV#$REV` becomes
   `git+duck://<chain>/forge/ducktape-industries/fluent31?rev=$REV#$REV`.
   Let Cargo write it (`cargo metadata` after step 1) and then READ the diff:
   it must be exactly those two lines. Any other changed line means Cargo
   re-resolved something; discard and find out why.
3. `crates/kernel/index-guest/testmap/guest.lock`: regenerate with
   `make fixture-guests`; its two fluent31 `source =` lines move the same way.

Acceptance, recorded in the PR body:

```sh
before=$(git show origin/dev:Cargo.lock | grep -A2 'name = "fluent-guest"$' | grep -o '#[0-9a-f]*"')
after=$(grep -A2 'name = "fluent-guest"$' Cargo.lock | grep -o '#[0-9a-f]*"')
[ "$before" = "$after" ]                      # the same commit
git diff origin/dev --stat -- Cargo.lock      # 2 insertions, 2 deletions
CARGO_HOME=$(mktemp -d) cargo build --locked  # a clean home fetches from Forge
cargo test --workspace && make fixture-guests-check
```

The clean `CARGO_HOME` matters: a warm one already holds the GitHub checkout
and would prove nothing about Forge. (Copy the user's Cargo `config.toml` with
`net.git-fetch-with-cli` into the temporary home first.)

## 3. The revert

`git revert` the flip commit, or edit the same three places back. The same
acceptance applies in the other direction: same commit hash, two-line lock
diff, `cargo build --locked` green from a clean `CARGO_HOME`. Because GitHub
never stopped being the origin of those commits, a revert needs nothing from
Forge and works with the node down.

## 4. Later and separate: the `platform` key

`[workspace.metadata.guest-builder] platform` is how guest-builder names THIS
repository when it clones it to build an index guest, and the testmap
`guest.lock` records it (`git+https://github.com/ducktape-industries/ducktape-sdk#<sha>`).
Flipping it makes every guest build depend on a reachable node, so it is its
own PR after section 2 has held for a while: mirror `ducktape-industries/ducktape-sdk`
(branch `dev`, because guest-builder's patch lines say `branch = "dev"`),
change the one `platform` line, regenerate `guest.lock` with
`make fixture-guests`, and require `make fixture-guests-check` to report
byte-identical artifacts. Downstream repositories that name sdk as a git
dependency flip in their own runbooks, in the order core, modules, views, app.

## What this runbook never does

No force-sync of a mirror, no credentials in any URL, manifest, lock or log,
no compat alias between the two spellings, no flip of more than one
repository in one PR, and no change to where issues, review or CI live.
