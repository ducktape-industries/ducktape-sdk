# ducktape-sdk — the standalone fixture guests + the index-guest test map.
#
# The fixture guests under crates/guests/ are plain-cargo wasm32 cdylibs.
# They are NEVER members of the root workspace (see its `exclude`): each is
# its own `[workspace]`, so it compiles alone and the componentizer wraps it
# with `guest-builder componentize`, never the normal `guest-builder
# <module-dir>` path (that path is for a crate that declares a `guest` or
# `index-guest` cargo feature and lives inside the platform's own graph).
#
# `crates/kernel/index-guest/testmap` DOES declare `index-guest`, so it goes
# through the ordinary path instead: `guest-builder --index` clones this
# repository at HEAD (pushed, clean) and builds the module out of that
# checkout, never out of the working tree.
#
# GUEST_BUILDER is prebuilt elsewhere; point this at your own checkout's copy.
GUEST_BUILDER ?= target/release/guest-builder
CARGO ?= cargo
FIXTURE_TARGET_DIR := $(CURDIR)/target/fixture-guests
TESTMAP_DIR := crates/kernel/index-guest/testmap

# id:crate-dir:cargo-features:core-crate-name:canonical-artifact[,extra-copy...]
# One shared target dir for all of them: wit-bindgen's tree compiles once
# instead of once per guest.
#
# `cut` rather than `$${x#*:}` on purpose: this is a variable definition, not
# a recipe, and make reads `#` here as the start of a comment — a parameter
# expansion silently truncates the function mid-body. `make -n` never catches
# it, because it never runs the shell.
FIXTURE_GUESTS := \
  hello:crates/guests/hello-wasm::hello_wasm:crates/guests/hello-wasm/component.wasm,crates/kernel/wasm-host/tests/fixtures/hello.component.wasm \
  hello-replacement:crates/guests/hello-wasm-replacement::hello_wasm_replacement:crates/guests/hello-wasm-replacement/component.wasm \
  noop:crates/guests/noop-wasm::noop_wasm:crates/guests/noop-wasm/component.wasm \
  sibling:crates/guests/sibling-wasm::sibling_wasm:crates/kernel/wasm-host/tests/fixtures/sibling.component.wasm \
  object:crates/guests/object-wasm::object_wasm:crates/kernel/wasm-host/tests/fixtures/object.component.wasm \
  object-replacement:crates/guests/object-wasm:replacement:object_wasm:crates/kernel/wasm-host/tests/fixtures/object-replacement.component.wasm

# Parse one FIXTURE_GUESTS record and build it. Sourced by every recipe that
# walks the list, so the parsing lives in one place.
FIXTURE_GUEST_SH = \
  fixture_parse() { \
    fg_id=$$(echo "$$1" | cut -d: -f1); \
    fg_dir=$$(echo "$$1" | cut -d: -f2); \
    fg_features=$$(echo "$$1" | cut -d: -f3); \
    fg_core=$$(echo "$$1" | cut -d: -f4); \
    fg_artifacts=$$(echo "$$1" | cut -d: -f5 | tr , ' '); \
  }; \
  fixture_build() { \
    fixture_parse "$$1"; \
    features=""; [ -z "$$fg_features" ] || features="--features $$fg_features"; \
    ( cd "$$fg_dir" && $(CARGO) build --locked --target-dir "$(FIXTURE_TARGET_DIR)" \
        --target wasm32-unknown-unknown --release $$features ) || return 1; \
    $(GUEST_BUILDER) componentize \
      "$(FIXTURE_TARGET_DIR)/wasm32-unknown-unknown/release/$$fg_core.wasm" --out "$$2"; \
  };

.PHONY: fixture-guests fixture-guests-check

## builds the standalone fixture guests and the index-guest test map,
## writing every committed artifact in place.
fixture-guests:
	@$(FIXTURE_GUEST_SH) \
	for rec in $(FIXTURE_GUESTS); do \
	  fixture_parse "$$rec"; \
	  canonical=$${fg_artifacts%% *}; \
	  echo "$(GUEST_BUILDER) componentize -> $$canonical"; \
	  fixture_build "$$rec" "$$canonical" || exit 1; \
	  for a in $$fg_artifacts; do \
	    [ "$$a" = "$$canonical" ] || cp "$$canonical" "$$a" || exit 1; \
	  done; \
	done
	$(GUEST_BUILDER) --index $(TESTMAP_DIR)

## the drift gate: rebuilds every fixture guest and the test map into a temp
## directory and byte-diffs the result against what is committed, touching
## nothing in the tree. Run after any change that could move their bytes
## (module-sdk, the module WIT, the toolchain pin, or a guest's own source).
fixture-guests-check:
	@$(FIXTURE_GUEST_SH) \
	tmp=$$(mktemp -d); \
	trap 'rm -rf "$$tmp"' EXIT; \
	stale=""; \
	for rec in $(FIXTURE_GUESTS); do \
	  fixture_parse "$$rec"; \
	  built="$$tmp/$$fg_id.component.wasm"; \
	  fixture_build "$$rec" "$$built" || exit 1; \
	  for a in $$fg_artifacts; do \
	    cmp -s "$$built" "$$a" || stale="$$stale $$a"; \
	  done; \
	done; \
	$(GUEST_BUILDER) --index $(TESTMAP_DIR) --out "$$tmp/testmap.index.wasm" >/dev/null || exit 1; \
	cmp -s "$$tmp/testmap.index.wasm" "$(TESTMAP_DIR)/index.wasm" || stale="$$stale $(TESTMAP_DIR)/index.wasm"; \
	if [ -z "$$stale" ]; then \
	  echo "fixture guest artifacts match a rebuild of their source"; \
	else \
	  echo "these committed guests do not match a rebuild of their source:$$stale"; \
	  exit 1; \
	fi

# What a wasm32 view may link. A wire crate that a view links must never reach
# the signing/identity graph (blst does not build for wasm32, and a view has no
# business holding keys); add a crate here when a view starts linking it.
VIEW_LINKABLE := duck-address refusal-class view-wire design chat-message boards-wire files-wire pages-wire duckfs-core
VIEW_FORBIDDEN := blst commonware-cryptography keyscheme identity-wire governance-wire

.PHONY: view-wasm-check

## builds every VIEW_LINKABLE crate for wasm32-unknown-unknown, then fails if
## the normal wasm32 dependency tree of any of them names a VIEW_FORBIDDEN
## crate (the build alone catches blst, not an identity crate that compiles).
view-wasm-check:
	@for crate in $(VIEW_LINKABLE); do \
	  $(CARGO) build --target wasm32-unknown-unknown -p $$crate || exit 1; \
	done; \
	reached=""; \
	for crate in $(VIEW_LINKABLE); do \
	  tree=$$($(CARGO) tree --target wasm32-unknown-unknown -e normal -p $$crate --prefix none) || exit 1; \
	  for dep in $(VIEW_FORBIDDEN); do \
	    if echo "$$tree" | grep -q "^$$dep v"; then reached="$$reached $$crate->$$dep"; fi; \
	  done; \
	done; \
	if [ -z "$$reached" ]; then \
	  echo "every view-linkable crate builds for wasm32 and stays off the signing/identity graph"; \
	else \
	  echo "view-linkable crates reach the signing/identity graph:$$reached"; \
	  exit 1; \
	fi
