//! `guest-builder` — build one module's wasm guest out of the platform
//! repository at a revision.
//!
//! a module carries its whole guest surface itself: a `src/guest.rs` behind a
//! wasm-only `guest` feature (the dispatch shell + the component export) over
//! the module SDK (`ducktape-module-sdk`). packaging that as a cdylib is
//! identical across modules — a manifest, a one-line lib, a `[workspace]`
//! table, the wasm32 patch set — so none of it is checked in: this tool
//! synthesizes it into a scratch workspace, builds for
//! `wasm32-unknown-unknown`, componentizes, and writes the artifact:
//!
//! ```text
//! guest-builder <module-dir> [--index] [--rev <sha>] [--platform <dir>]
//!               [--out <artifact.wasm>] [--scratch <dir>]
//! guest-builder componentize <core.wasm> --out <component.wasm>
//! guest-builder vendor --out <dir> [--platform <dir>] [--directory <path>]
//! ```
//!
//! # the platform is a workspace, and it names itself
//!
//! THE PLATFORM IS THE WORKSPACE THE COMMAND RUNS IN — the one `cargo`
//! resolves from the working directory, or the one `--platform <dir>` names.
//! Never a path relative to this binary: the module set lives in other
//! repositories now (the system modules in `ducktape`, the app modules in
//! `ducktape-modules`), and a builder that could only read its own tree could
//! build none of them.
//!
//! that workspace's root manifest says which repository it IS:
//!
//! ```toml
//! [workspace.metadata.guest-builder]
//! platform = "https://github.com/ducktape-industries/ducktape"
//! ```
//!
//! the URL is part of every symbol hash, so it is written once, in the tree it
//! names, rather than guessed from a checkout's remote — two builders spelling
//! it differently would produce different bytes for the same revision. a
//! workspace without the key builds no guest, and says so.
//!
//! the shell's ONE dependency is the module, reached out of that repository as
//! a git source at the revision the shell lock pins — never out of the
//! checkout in place. that is what makes a module independently buildable and
//! its bytes reproducible: the build inputs are the module's revision, its
//! lock, the module SDK revision its platform pins, and the toolchain — the
//! rust channel `rust-toolchain.toml` pins and the componentizer this crate
//! links (`wit-component`, pinned in its manifest; see the crate root) — and
//! nothing else.
//!
//! * every crate of that repository the module reads (a sibling module, its
//!   own wire types) resolves inside that one git source at that one revision —
//!   a path dependency inside a git checkout IS the git source — so a module's
//!   platform is one revision by construction.
//! * THE MODULE SDK IS ITS HOST'S. `ducktape-module-sdk`, and with it the
//!   wasm32 patch stubs beside it, come from `ducktape-sdk` — a second
//!   repository, which the platform's own `Cargo.lock` pins to a revision.
//!   the shell resolves the same source and then pins the same revision
//!   ([`ModuleSdk`]), so a guest can never speak an ABI its host does not: the
//!   host links that revision's `sdk`, the guest compiles that revision's
//!   bindings.
//! * a git source's location is no part of a symbol hash (a path dependency's
//!   absolute location is), so bytes do not depend on where a checkout lives.
//!   the directory cargo unpacks the revision into is remapped out of panic
//!   paths by [`remap_flags`].
//! * the shell names the module WITHOUT a `rev`: cargo hashes a git reference
//!   as written into `-C metadata`, so a revision in the manifest would change
//!   every symbol name — and every artifact's bytes — on every commit. the
//!   revision lives in the lock, which is not hashed, so a module rebuilt
//!   at a later revision that changed none of the sources it compiles yields
//!   byte-identical output.
//! * the shell lock is the module's `guest.lock`, committed beside its
//!   artifacts: the record of the revision and the registry versions an
//!   artifact came from, and the seed of the next build, so a crates.io
//!   publish between two rebuilds does not move the bytes. artifact and lock
//!   are ALWAYS written together: a canonical build puts them in the module
//!   directory, an `--out` build puts them side by side at the out path
//!   (`<out>` and `<out>.lock`, so `runs.component.wasm` is joined by
//!   `runs.component.lock`). the module directory stays untouched either way,
//!   which is what lets the drift check rebuild every guest without dirtying
//!   the tree — and what lets it hand the result back without a second build,
//!   since a lock that does not describe the bytes beside it is worse than no
//!   lock at all.
//!
//! the revision defaults to the platform checkout's HEAD and must be
//! reachable at the URL that workspace names: push before building.
//! uncommitted inputs anywhere in the resolved platform graph (the module,
//! its siblings, the workspace manifest and the lock that pins the SDK) are
//! refused, since the build would silently compile HEAD instead.
//!
//! `--index` builds the module's INDEX guest instead: the fluentabi mapper
//! behind the crate's `index-guest` feature (a `src/index_guest.rs` — see the
//! index-guest crate). same shell, a second member, and the artifact stays
//! core wasm (`index.wasm`, no componentize step): the fluent31 engine
//! executes plain wasm32 modules, not components.
//!
//! a module authored outside this repository needs none of the shell: its
//! crate is the cdylib, it pins `ducktape-module-sdk` and the patch stubs by
//! git revision in its own manifest, and plain cargo + the `componentize`
//! verb (or `wasm-tools component new` at the same release) build it.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

const USAGE: &str = "usage: guest-builder <module-dir> [--index] [--rev <sha>] \
     [--platform <dir>] [--out <artifact.wasm>] [--scratch <dir>]\n       \
     guest-builder componentize <core.wasm> --out <component.wasm>\n       \
     guest-builder vendor --out <dir> [--platform <dir>] \
     [--directory <path the config names>]";

/// the key a platform workspace names itself with, in its root manifest.
const PLATFORM_KEY: &str = "[workspace.metadata.guest-builder] platform";

/// which of a module's two guests to build. the consensus component and the
/// index mapper share the shell; everything guest-specific — contract
/// feature, shell member, artifact name, whether the cdylib is componentized —
/// hangs off this one discriminant.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GuestKind {
    /// the `ducktape:module` consensus component (`guest` feature).
    Component,
    /// the fluentabi index mapper (`index-guest` feature), core wasm.
    Index,
}

impl GuestKind {
    const ALL: [GuestKind; 2] = [GuestKind::Component, GuestKind::Index];

    fn feature(self) -> &'static str {
        match self {
            GuestKind::Component => "guest",
            GuestKind::Index => "index-guest",
        }
    }

    fn artifact(self) -> &'static str {
        match self {
            GuestKind::Component => "component.wasm",
            GuestKind::Index => "index.wasm",
        }
    }

    /// the shell workspace member (and the cdylib's name suffix) for this guest.
    fn member(self) -> &'static str {
        match self {
            GuestKind::Component => "component",
            GuestKind::Index => "index",
        }
    }

    fn missing_feature_hint(self, name: &str) -> String {
        match self {
            GuestKind::Component => format!(
                "module `{name}` declares no `guest` feature — the port lives in the \
                 module crate (a `src/guest.rs` behind `guest = [\"dep:ducktape-module-sdk\"]`); \
                 see crates/modules/apps/tasks for the shape"
            ),
            GuestKind::Index => format!(
                "module `{name}` declares no `index-guest` feature — the index mapper \
                 lives in the module crate (a `src/index_guest.rs` behind \
                 `index-guest = [\"index_guest/guest\"]`); see crates/modules/apps/tasks \
                 for the shape"
            ),
        }
    }
}

fn main() {
    let Err(err) = run() else { return };
    eprintln!("guest-builder: {err}");
    process::exit(1);
}

fn run() -> Result<(), String> {
    match parse_args()? {
        Args::Build(args) => build_guest(args),
        Args::Componentize { core, out } => componentize(&core, &out),
        Args::Vendor {
            out,
            directory,
            platform,
        } => vendor(&out, directory.as_deref(), platform.as_deref()),
    }
}

fn build_guest(args: BuildArgs) -> Result<(), String> {
    let kind = args.kind;
    let platform = platform(args.platform.as_deref())?;
    let module_dir = canonical(&args.module_dir)?;
    let module = read_module(&platform.root, &module_dir)?;
    let declares_requested_guest = module.guests.contains(&kind);
    if !declares_requested_guest {
        return Err(kind.missing_feature_hint(&module.name));
    }

    let rev = match &args.rev {
        Some(rev) => rev.clone(),
        None => head(&platform.root)?,
    };
    let sdk = module_sdk(&platform.root)?;

    let scratch = match args.scratch {
        Some(dir) => dir,
        None => platform
            .root
            .join("target/guest-builder")
            .join(&module.name),
    };
    eprintln!(
        "guest-builder: {} {} at {rev}{}",
        module.name,
        kind.artifact(),
        sdk.note()
    );
    seed_lock(&scratch, &module_dir)?;
    let graph = pin(&scratch, &module, &platform.git, &rev, &sdk)?;
    let checkout = checkout_root(&graph, &module)?;
    let builds_head = args.rev.is_none();
    if builds_head {
        let inputs = platform_inputs(&graph, &checkout, &platform.git)?;
        refuse_modified_sources(&platform.root, &inputs)?;
    }
    build(
        &scratch,
        &module.name,
        kind,
        &remap_flags(&scratch, &graph)?,
    )?;

    let cdylib = cdylib_path(&scratch, &module.name, kind);
    // artifact and lock travel together, wherever they land: the lock is the
    // record of THOSE bytes, and one without the other is a half-answer. a
    // one-off `--out` build still leaves the MODULE DIRECTORY untouched — it
    // writes the pair at the out path instead — so a check that rebuilds every
    // guest keeps the tree clean AND can hand its result back without paying
    // for the same build twice.
    let out = match args.out {
        Some(path) => {
            write_artifact(kind, &cdylib, &path)?;
            write_lock(&scratch, &path.with_extension("lock"))?;
            path
        }
        None => {
            let canonical = module_dir.join(kind.artifact());
            write_artifact(kind, &cdylib, &canonical)?;
            write_lock(&scratch, &module_dir.join("guest.lock"))?;
            canonical
        }
    };
    println!("{}", out.display());
    Ok(())
}

fn write_artifact(kind: GuestKind, cdylib: &Path, out: &Path) -> Result<(), String> {
    match kind {
        GuestKind::Component => componentize(cdylib, out),
        GuestKind::Index => copy_cdylib(cdylib, out),
    }
}

// ============================================================================
// argument parsing
// ============================================================================

/// the two things the tool does: build a module's guest out of the
/// repository, or wrap one already-built core module as a component.
enum Args {
    Build(BuildArgs),
    Componentize {
        core: PathBuf,
        out: PathBuf,
    },
    Vendor {
        out: PathBuf,
        /// what the emitted config names as the vendor directory, when that is
        /// not where this run writes it — a guest image builds the set on the
        /// host and mounts it somewhere else entirely.
        directory: Option<String>,
        platform: Option<PathBuf>,
    },
}

struct BuildArgs {
    module_dir: PathBuf,
    kind: GuestKind,
    rev: Option<String>,
    out: Option<PathBuf>,
    scratch: Option<PathBuf>,
    /// the platform workspace to work in, when it is not the one the working
    /// directory sits in.
    platform: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut argv = env::args().skip(1).peekable();
    // step: the verb ladder became a match when the third verb arrived. ONE
    // discriminant — the leading word — and the build verb is the unnamed one,
    // which is why it is the fallthrough rather than an arm.
    let verb = argv.peek().cloned().unwrap_or_default();
    match verb.as_str() {
        "componentize" => {
            argv.next();
            parse_componentize_args(argv)
        }
        "vendor" => {
            argv.next();
            parse_vendor_args(argv)
        }
        _ => parse_build_args(argv).map(Args::Build),
    }
}

fn parse_vendor_args(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut out = None;
    let mut directory = None;
    let mut platform = None;
    while let Some(arg) = argv.next() {
        let value = argv
            .next()
            .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))?;
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value)),
            "--directory" => directory = Some(value),
            "--platform" => platform = Some(PathBuf::from(value)),
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let Some(out) = out else {
        return Err(format!("vendor needs --out <dir>\n{USAGE}"));
    };
    Ok(Args::Vendor {
        out,
        directory,
        platform,
    })
}

fn parse_componentize_args(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut core = None;
    let mut out = None;
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--out" => {
                out = Some(PathBuf::from(
                    argv.next()
                        .ok_or_else(|| format!("--out needs a value\n{USAGE}"))?,
                ));
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown flag {flag}\n{USAGE}"));
            }
            positional => {
                let unclaimed = core.is_none();
                if !unclaimed {
                    return Err(format!("unexpected argument {positional}\n{USAGE}"));
                }
                core = Some(PathBuf::from(positional));
            }
        }
    }
    let (Some(core), Some(out)) = (core, out) else {
        return Err(USAGE.to_string());
    };
    Ok(Args::Componentize { core, out })
}

fn parse_build_args(mut argv: impl Iterator<Item = String>) -> Result<BuildArgs, String> {
    let mut module_dir = None;
    let mut kind = GuestKind::Component;
    let mut rev = None;
    let mut out = None;
    let mut scratch = None;
    let mut platform = None;

    while let Some(arg) = argv.next() {
        let flag_value = |argv: &mut dyn Iterator<Item = String>| {
            argv.next()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--index" => kind = GuestKind::Index,
            "--rev" => rev = Some(flag_value(&mut argv)?),
            "--out" => out = Some(PathBuf::from(flag_value(&mut argv)?)),
            "--scratch" => scratch = Some(PathBuf::from(flag_value(&mut argv)?)),
            "--platform" => platform = Some(PathBuf::from(flag_value(&mut argv)?)),
            flag if flag.starts_with("--") => {
                return Err(format!("unknown flag {flag}\n{USAGE}"));
            }
            positional => {
                let unclaimed = module_dir.is_none();
                if !unclaimed {
                    return Err(format!("unexpected argument {positional}\n{USAGE}"));
                }
                module_dir = Some(PathBuf::from(positional));
            }
        }
    }

    let Some(module_dir) = module_dir else {
        return Err(USAGE.to_string());
    };
    Ok(BuildArgs {
        module_dir,
        kind,
        rev,
        out,
        scratch,
        platform,
    })
}

// ============================================================================
// the platform — the workspace a run works in, and what it says about itself
// ============================================================================

/// the workspace a guest is built out of: the tree its module crate lives in,
/// and the repository that tree publishes itself as.
struct Platform {
    root: PathBuf,
    /// `[workspace.metadata.guest-builder] platform`: the git URL the shell
    /// reaches the module through.
    git: String,
}

/// the package name the platform pins its module SDK with.
const MODULE_SDK: &str = "ducktape-module-sdk";

/// the workspace `dir` sits in, or the working directory's when no
/// `--platform` was given. `cargo metadata --no-deps` answers both questions
/// in one call and resolves no dependencies to do it: which workspace this is,
/// and what it calls itself.
fn platform(dir: Option<&Path>) -> Result<Platform, String> {
    let anchor = match dir {
        Some(dir) => canonical(dir)?,
        None => env::current_dir().map_err(|e| format!("reading the working directory: {e}"))?,
    };
    let output = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&anchor)
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "no cargo workspace at {}: {}",
            anchor.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parsing cargo metadata output: {e}"))?;
    let Some(root) = meta["workspace_root"].as_str() else {
        return Err("cargo metadata names no workspace root".to_string());
    };
    let Some(git) = meta["metadata"]["guest-builder"]["platform"].as_str() else {
        return Err(format!(
            "{root}/Cargo.toml declares no `{PLATFORM_KEY}`. a workspace a guest is built \
             out of names the repository that guest is published from:\n\n    \
             [workspace.metadata.guest-builder]\n    \
             platform = \"https://github.com/ducktape-industries/ducktape\"\n\n\
             the URL is hashed into every guest symbol, so it is written in the tree it \
             names rather than read off whatever a checkout calls its remote"
        ));
    };
    Ok(Platform {
        root: canonical(Path::new(root))?,
        git: git.to_string(),
    })
}

/// where a guest's module SDK — and with it the wasm32 patch stubs beside it —
/// comes from.
enum ModuleSdk {
    /// a second repository, at the revision the platform's own `Cargo.lock`
    /// pins: the host that loads the guest links that revision's `sdk`, so the
    /// guest compiles that revision's bindings. a guest whose SDK is not its
    /// host's speaks an ABI its host does not.
    Pinned(SdkPin),
    /// the platform IS the SDK's repository — ducktape-sdk builds a guest of
    /// its own, the reference index mapper — so the SDK rides the module's own
    /// source and revision, and there is nothing to pin.
    ThePlatform,
}

impl ModuleSdk {
    /// how the shell spells the SDK source, given how it spells the module's.
    fn source(&self, module_source: &str) -> String {
        match self {
            ModuleSdk::Pinned(pin) => pin.source.clone(),
            ModuleSdk::ThePlatform => module_source.to_string(),
        }
    }

    fn note(&self) -> String {
        match self {
            ModuleSdk::Pinned(pin) => format!(", module SDK {}", pin.rev),
            ModuleSdk::ThePlatform => String::new(),
        }
    }
}

struct SdkPin {
    /// the dependency spelling — `git = "<url>"` plus the reference the
    /// platform names (`branch = "dev"`) — so the shell's patch stubs and the
    /// module's own SDK dependency name ONE cargo source, and cargo keeps one
    /// checkout of it at one revision.
    source: String,
    rev: String,
}

fn module_sdk(platform_root: &Path) -> Result<ModuleSdk, String> {
    let path = platform_root.join("Cargo.lock");
    let lock = fs::read_to_string(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let Some(source) = locked_source(&lock, MODULE_SDK) else {
        return Ok(ModuleSdk::ThePlatform);
    };
    sdk_pin_of(&source)
        .map(ModuleSdk::Pinned)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// the `source = "…"` a lock records for one package.
fn locked_source(lock: &str, package: &str) -> Option<String> {
    let name = format!("name = {package:?}");
    let entry = lock
        .split("[[package]]")
        .find(|entry| entry.lines().any(|line| line == name))?;
    let source = entry
        .lines()
        .find_map(|line| line.strip_prefix("source = \"")?.strip_suffix('"'))?;
    Some(source.to_string())
}

/// `git+<url>[?<reference>]#<sha>` — a lock's git source — as the dependency
/// spelling and revision a shell pins it with.
fn sdk_pin_of(source: &str) -> Result<SdkPin, String> {
    let Some(locator) = source.strip_prefix("git+") else {
        return Err(format!("{MODULE_SDK} is not a git source: {source}"));
    };
    let Some((locator, rev)) = locator.split_once('#') else {
        return Err(format!("{MODULE_SDK}'s source names no revision: {source}"));
    };
    let (url, reference) = match locator.split_once('?') {
        None => (locator, String::new()),
        Some((url, query)) => {
            let Some((key, value)) = query.split_once('=') else {
                return Err(format!(
                    "{MODULE_SDK}'s source reference is not key=value: {source}"
                ));
            };
            (url, format!(", {key} = {value:?}"))
        }
    };
    Ok(SdkPin {
        source: format!("git = {url:?}{reference}"),
        rev: rev.to_string(),
    })
}

// ============================================================================
// module introspection — name, place in the repository, declared guests
// ============================================================================

struct Module {
    name: String,
    /// the module directory relative to the platform root: its place in the
    /// repository, which is where the build reads it from.
    path: PathBuf,
    /// the guests the crate declares, by contract feature.
    guests: Vec<GuestKind>,
}

/// read the module's package name and contract features via `cargo metadata`
/// on the working-tree manifest. the build compiles the repository, so the
/// module must be in it; a crate outside the platform checkout is an
/// out-of-tree module, which is its own cdylib and needs no shell.
fn read_module(platform_root: &Path, module_dir: &Path) -> Result<Module, String> {
    let Ok(path) = module_dir.strip_prefix(platform_root) else {
        return Err(format!(
            "{} is outside the platform workspace {} — run in the workspace that owns the \
             module, or name it with --platform <dir>. a module authored outside any \
             ducktape workspace is its own cdylib crate pinning ducktape-module-sdk by git \
             revision, built with cargo and `guest-builder componentize` directly",
            module_dir.display(),
            platform_root.display()
        ));
    };
    let manifest = module_dir.join("Cargo.toml");
    let output = Command::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata on {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parsing cargo metadata output: {e}"))?;

    // metadata on a workspace member lists every member; the module is the
    // package whose manifest is the one we asked about.
    let is_this_module =
        |pkg: &&serde_json::Value| pkg["manifest_path"].as_str() == manifest.to_str();
    let Some(pkg) = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(is_this_module)
    else {
        return Err(format!("{} is not a cargo package", module_dir.display()));
    };
    let Some(name) = pkg["name"].as_str() else {
        return Err(format!(
            "{}: package name missing from metadata",
            manifest.display()
        ));
    };
    let guests = GuestKind::ALL
        .into_iter()
        .filter(|kind| pkg["features"].get(kind.feature()).is_some())
        .collect();
    Ok(Module {
        name: name.to_string(),
        path: path.to_path_buf(),
        guests,
    })
}

/// the checkout's HEAD: the revision a build without `--rev` compiles.
fn head(platform_root: &Path) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(platform_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| format!("running git rev-parse: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD in {}: {}",
            platform_root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Every local source used by the platform graph must agree with HEAD. The
/// generated artifacts and lock are outputs, so a rebuild may rewrite them.
fn refuse_modified_sources(platform_root: &Path, inputs: &BTreeSet<PathBuf>) -> Result<(), String> {
    let mut changed = BTreeSet::new();
    for args in [
        vec!["diff", "--name-only", "-z", "HEAD", "--"],
        vec!["ls-files", "--others", "--exclude-standard", "-z", "--"],
    ] {
        let output = Command::new("git")
            .arg("-C")
            .arg(platform_root)
            .args(args)
            .args(inputs)
            .args([
                ":(exclude)**/component.wasm",
                ":(exclude)**/index.wasm",
                ":(exclude)**/guest.lock",
            ])
            .output()
            .map_err(|e| format!("checking platform sources: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "checking platform sources: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        changed.extend(
            output
                .stdout
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned()),
        );
    }
    if changed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "uncommitted platform build inputs: {} — the guest compiles HEAD; commit and push these sources first (or pass --rev to build a specific revision)",
        changed.into_iter().collect::<Vec<_>>().join(", ")
    ))
}

/// Cargo's resolved platform packages include the module and its siblings.
/// Workspace manifests and build configuration are inputs even though Cargo
/// does not report them as packages — and so is the lock, which is what says
/// at which revision this guest compiles the module SDK.
fn platform_inputs(
    graph: &serde_json::Value,
    checkout: &Path,
    git: &str,
) -> Result<BTreeSet<PathBuf>, String> {
    let mut inputs = BTreeSet::from([
        PathBuf::from("Cargo.toml"),
        PathBuf::from("Cargo.lock"),
        PathBuf::from("rust-toolchain.toml"),
        PathBuf::from(".cargo"),
    ]);
    let Some(packages) = graph["packages"].as_array() else {
        return Err("cargo metadata has no packages".to_string());
    };
    for package in packages {
        // the source ID is `git+<url>` followed by `?<reference>` or `#<sha>`:
        // matching the URL alone would also claim a repository whose name
        // merely starts with the platform's.
        let from_platform = package["source"]
            .as_str()
            .and_then(|source| source.strip_prefix("git+")?.strip_prefix(git))
            .is_some_and(|rest| rest.starts_with('?') || rest.starts_with('#'));
        if !from_platform {
            continue;
        }
        let Some(manifest) = package["manifest_path"].as_str() else {
            return Err("platform package has no manifest path".to_string());
        };
        let path = Path::new(manifest)
            .strip_prefix(checkout)
            .map_err(|e| format!("platform package outside its checkout: {manifest}: {e}"))?;
        let Some(directory) = path.parent() else {
            return Err(format!("platform manifest has no directory: {manifest}"));
        };
        inputs.insert(directory.to_path_buf());
    }
    Ok(inputs)
}

// ============================================================================
// synthesis — the shell workspace every module shares
// ============================================================================

/// write the shell workspace: one cdylib member per guest the module
/// declares, each depending on the module alone (its contract feature on,
/// defaults off) out of the platform git source, plus the uniform wasm32 patch
/// set out of the module SDK's. regenerated on every run — nothing here is
/// hand-maintained state, except the lock, which is seeded from the module's
/// committed `guest.lock` when there is one.
fn synthesize(scratch: &Path, module: &Module, source: &str, sdk: &str) -> Result<(), String> {
    for kind in &module.guests {
        let member = scratch.join(kind.member());
        let src = member.join("src");
        fs::create_dir_all(&src).map_err(|e| format!("creating {}: {e}", src.display()))?;
        write(
            &member.join("Cargo.toml"),
            &member_manifest(&module.name, *kind, source),
        )?;
        write(&src.join("lib.rs"), &member_lib(&module.name))?;
    }
    write(
        &scratch.join("Cargo.toml"),
        &workspace_manifest(&module.guests, sdk),
    )
}

fn seed_lock(scratch: &Path, module_dir: &Path) -> Result<(), String> {
    fs::create_dir_all(scratch).map_err(|e| format!("creating {}: {e}", scratch.display()))?;
    let lock = scratch.join("Cargo.lock");
    let committed = module_dir.join("guest.lock");
    let Err(error) = fs::copy(&committed, &lock) else {
        return Ok(());
    };
    let seed_is_absent = error.kind() == std::io::ErrorKind::NotFound;
    if !seed_is_absent {
        return Err(format!(
            "seeding lock from {}: {error}",
            committed.display()
        ));
    }
    // A first build has no seed. A previous scratch lock is never an input.
    let Err(error) = fs::remove_file(&lock) else {
        return Ok(());
    };
    let scratch_lock_is_absent = error.kind() == std::io::ErrorKind::NotFound;
    if scratch_lock_is_absent {
        return Ok(());
    }
    Err(format!("removing scratch lock: {error}"))
}

fn workspace_manifest(guests: &[GuestKind], sdk: &str) -> String {
    let members: Vec<String> = guests
        .iter()
        .map(|kind| format!("\"{}\"", kind.member()))
        .collect();
    format!(
        r#"# synthesized by guest-builder — do not edit; regenerated on every build.
# the packaging shell only: the module logic and its guest ports live in the
# module crate, read out of the platform repository at the revision the lock
# pins. one member per guest the module declares.
[workspace]
members = [{members}]
resolver = "2"

# a guest is optimized as ONE unit. without this the shell inherits cargo's
# release defaults — lto = false, codegen-units = 16 — and every crate boundary
# inside a module's own graph becomes an optimization barrier, so splitting a
# module's wire types into their own crate costs the component real bytes for
# no change in behaviour. the module set is shipped, hashed and consensus-
# pinned, so it is compiled like something shipped rather than something built
# in a loop.
#
# The view guest profile carries two more settings this one deliberately does
# NOT take (sdk#2), so that reading the two side by side is not an invitation
# to finish the job:
#   * `strip = true` deletes the wasm name section, and that section is what
#     turns a validator's trap log from an address into a function name. A
#     module guest runs on every validator of every network; the operator
#     holding that log line is not the person who can rebuild the module
#     unstripped.
#   * `panic = "abort"` is not a lever on the same axis as the three above.
#     opt-level, lto and codegen-units produce a different ENCODING of the same
#     program, so a rebuild at other values is the same module. `abort`
#     produces a different program.
[profile.release]
opt-level = 3
lto = "fat"
codegen-units = 1

{patches}"#,
        members = members.join(", "),
        patches = patch_section(sdk)
    )
}

/// the uniform wasm32 patch set: the crates a guest substitutes because they
/// cannot compile to wasm32, out of `crates/module-sdk/stubs` in the module
/// SDK's repository at the revision the platform pins. Spelled against the
/// SAME source as the module's own `ducktape-module-sdk` dependency, so one
/// checkout at one revision serves both. Applied to every guest; cargo's
/// "unused patch" warning on a module whose graph never pulls one of these
/// crates is expected and harmless.
fn patch_section(sdk: &str) -> String {
    format!(
        r#"
[patch.crates-io]
getrandom-02 = {{ package = "getrandom", version = "0.2", {sdk} }}
getrandom-03 = {{ package = "getrandom", version = "0.3", {sdk} }}
getrandom-04 = {{ package = "getrandom", version = "0.4", {sdk} }}
blst = {{ {sdk} }}
"#
    )
}

/// The member manifest uses an explicit revision during resolution, then
/// source alone during compilation: rustc must never hash a written selector.
fn member_manifest(name: &str, kind: GuestKind, source: &str) -> String {
    let feature = kind.feature();
    let member = kind.member();
    format!(
        r#"# synthesized by guest-builder — do not edit; regenerated on every build.
[package]
name = "{name}-{member}"
version = "0.0.0"
edition = "2021"
publish = false

[lib]
crate-type = ["cdylib"]

[dependencies]
{name} = {{ {source}, default-features = false, features = ["{feature}"] }}
"#
    )
}

fn member_lib(name: &str) -> String {
    format!(
        "// synthesized by guest-builder — link the module crate for its guest export.\n\
         extern crate {} as _;\n",
        snake(name)
    )
}

/// Resolve against the requested revision from the first lookup, including
/// a module that does not exist on the repository's default branch. The
/// explicit selector is removed from both manifests and lock before rustc
/// runs: only the lock's precise commit may vary between identical builds.
///
/// The module SDK is pinned the other way round — by the lock alone. Its
/// source is spelled exactly as the platform spells it (a branch, usually), so
/// resolution lands on that branch's head; `cargo update --precise` then moves
/// the whole source back to the revision the platform runs. Writing the
/// revision into the shell instead would make a second cargo source out of the
/// one the module itself depends on, and the guest would carry two SDKs.
///
/// The returned graph is the FINAL one: resolved, pinned, and re-read under
/// `--locked`, so nothing that follows reads a path or a revision the build
/// will not use.
fn pin(
    scratch: &Path,
    module: &Module,
    git: &str,
    rev: &str,
    sdk: &ModuleSdk,
) -> Result<serde_json::Value, String> {
    let locked_source = format!("git = {git:?}");
    let revision_source = format!("{locked_source}, rev = {rev:?}");
    synthesize(
        scratch,
        module,
        &revision_source,
        &sdk.source(&revision_source),
    )?;
    let resolve = |locked: bool| -> Result<serde_json::Value, String> {
        let mut command = Command::new(cargo());
        command.args([
            "metadata",
            "--format-version",
            "1",
            "--filter-platform",
            "wasm32-unknown-unknown",
        ]);
        if locked {
            command.arg("--locked");
        }
        let output = command
            .current_dir(scratch)
            .output()
            .map_err(|e| format!("resolving guest dependencies: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "resolving {} at {rev} from {git} failed (push the revision first): {}",
                module.name,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("parsing cargo metadata output: {e}"))
    };
    resolve(false)?;
    pin_module_sdk(scratch, sdk)?;

    let lock = scratch.join("Cargo.lock");
    let content = fs::read_to_string(&lock).map_err(|e| format!("reading resolved lock: {e}"))?;
    let selected_source = format!("git+{git}?rev={rev}");
    let precise_source = format!("git+{git}");
    // Cargo uses this source ID in package entries and disambiguated dependency
    // strings. Normalize every occurrence so they continue to name one source.
    write(&lock, &content.replace(&selected_source, &precise_source))?;
    synthesize(scratch, module, &locked_source, &sdk.source(&locked_source))?;
    resolve(true)
}

/// Move the module SDK's source to the revision the platform pins. A guest
/// that declares no SDK dependency at all — an index mapper over the engine
/// ABI — has nothing to move, and neither has one whose SDK is the platform
/// it is already being built at.
fn pin_module_sdk(scratch: &Path, sdk: &ModuleSdk) -> Result<(), String> {
    let ModuleSdk::Pinned(sdk) = sdk else {
        return Ok(());
    };
    let lock = scratch.join("Cargo.lock");
    let content = fs::read_to_string(&lock).map_err(|e| format!("reading resolved lock: {e}"))?;
    let Some(resolved) = locked_source(&content, MODULE_SDK) else {
        return Ok(());
    };
    let already_the_platform_revision = resolved.ends_with(&format!("#{}", sdk.rev));
    if already_the_platform_revision {
        return Ok(());
    }
    let output = Command::new(cargo())
        .args(["update", "-p", MODULE_SDK, "--precise", &sdk.rev])
        .current_dir(scratch)
        .output()
        .map_err(|e| format!("pinning {MODULE_SDK}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "pinning {MODULE_SDK} to {} (the revision the platform's Cargo.lock names): {}",
            sdk.rev,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

/// Locate the platform checkout from the module's resolved manifest.
fn checkout_root(meta: &serde_json::Value, module: &Module) -> Result<PathBuf, String> {
    let is_the_module_from_git = |pkg: &&serde_json::Value| {
        let named = pkg["name"].as_str() == Some(module.name.as_str());
        let from_git = pkg["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("git+"));
        named && from_git
    };
    let Some(pkg) = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(is_the_module_from_git)
    else {
        return Err(format!(
            "{} is not in the shell's resolved graph as a git package",
            module.name
        ));
    };
    let Some(manifest_path) = pkg["manifest_path"].as_str() else {
        return Err(format!(
            "{}: manifest path missing from metadata",
            module.name
        ));
    };
    checkout_root_of(Path::new(manifest_path), &module.path)
}

/// the checkout a git package's manifest sits in: its manifest path minus the
/// module's place in the repository.
fn checkout_root_of(manifest_path: &Path, module_path: &Path) -> Result<PathBuf, String> {
    let module_manifest = module_path.join("Cargo.toml");
    let depth = module_manifest.components().count();
    let Some(root) = manifest_path.ancestors().nth(depth) else {
        return Err(format!(
            "{} is shallower than {}",
            manifest_path.display(),
            module_manifest.display()
        ));
    };
    let sits_at_its_repository_place = root.join(&module_manifest) == manifest_path;
    if !sits_at_its_repository_place {
        return Err(format!(
            "{} does not end in {}",
            manifest_path.display(),
            module_manifest.display()
        ));
    }
    Ok(root.to_path_buf())
}

// ============================================================================
// build + componentize
// ============================================================================

/// `--remap-path-prefix` mappings that keep every builder-local absolute path
/// out of the artifact's CONTENT — panic locations name their source file, so
/// without these the bytes carry the builder's `/home/<user>/...` around
/// forever. `ops/wasm-repro-check.sh` and the host-path scan in
/// `make wasm-modules-check` are the gates.
///
/// the checkout mappings come last on purpose: a checkout sits under
/// CARGO_HOME and rustc takes the LAST matching mapping, so each
/// revision-specific `git/checkouts/<repo>-<hash>/<rev>` directory becomes the
/// stable `/<repo>` — the same token at every revision — instead of a path
/// that would move the bytes on every commit to it. EVERY git checkout the
/// guest compiles gets one: the platform's, the module SDK's, and fluent31's
/// under an index guest.
fn remap_flags(scratch: &Path, graph: &serde_json::Value) -> Result<String, String> {
    let home = env::var("HOME").unwrap_or_default();
    let tool_home =
        |key: &str, dir: &str| env::var(key).unwrap_or_else(|_| format!("{home}/{dir}"));
    let mut mappings = vec![
        (tool_home("CARGO_HOME", ".cargo"), "/cargo".to_string()),
        (tool_home("RUSTUP_HOME", ".rustup"), "/rustup".to_string()),
        (scratch.display().to_string(), "/guest-builder".to_string()),
    ];
    mappings.extend(git_checkouts(graph)?);
    let flags: Vec<String> = mappings
        .iter()
        .map(|(from, to)| format!("--remap-path-prefix={from}={to}"))
        .collect();
    // the ENCODED form's separator: plain `RUSTFLAGS` splits on whitespace, so
    // a path containing a space would tear one flag into two.
    Ok(flags.join("\x1f"))
}

/// every git checkout the resolved graph compiles out of, and the stable token
/// its revision-named directory is remapped to: `<repo>-<url hash>/<rev>`
/// becomes `/<repo>`. Sorted, so the flag order is the graph's rather than
/// cargo's.
fn git_checkouts(graph: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    let Some(packages) = graph["packages"].as_array() else {
        return Err("cargo metadata has no packages".to_string());
    };
    let mut checkouts = BTreeSet::new();
    for package in packages {
        let from_git = package["source"]
            .as_str()
            .is_some_and(|source| source.starts_with("git+"));
        if !from_git {
            continue;
        }
        let Some(manifest) = package["manifest_path"].as_str() else {
            return Err("git package has no manifest path".to_string());
        };
        let Some(checkout) = git_checkout_of(Path::new(manifest)) else {
            return Err(format!("{manifest} is not under a cargo git checkout"));
        };
        let Some(repository) = checkout
            .parent()
            .and_then(|dir| dir.file_name())
            .and_then(|name| name.to_str())
            .and_then(|name| name.rsplit_once('-'))
        else {
            return Err(format!("{} is not a cargo checkout", checkout.display()));
        };
        checkouts.insert((checkout.display().to_string(), format!("/{}", repository.0)));
    }
    Ok(checkouts.into_iter().collect())
}

/// the `<cargo home>/git/checkouts/<repo>-<hash>/<rev>` a manifest sits under:
/// the directory named for the revision, which is the one whose name must not
/// reach the bytes.
fn git_checkout_of(manifest: &Path) -> Option<&Path> {
    let checkouts_is_the_grandparent = |dir: &&Path| {
        dir.parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|name| name == "checkouts")
    };
    manifest.ancestors().find(checkouts_is_the_grandparent)
}

fn build(scratch: &Path, name: &str, kind: GuestKind, rustflags: &str) -> Result<(), String> {
    let status = build_command(scratch, name, kind, rustflags)
        .status()
        .map_err(|e| format!("running cargo build: {e}"))?;
    if !status.success() {
        return Err(format!("wasm32 build failed in {}", scratch.display()));
    }
    Ok(())
}

fn build_command(scratch: &Path, name: &str, kind: GuestKind, rustflags: &str) -> Command {
    let member = format!("{name}-{}", kind.member());
    let mut command = Command::new(cargo());
    command
        // `--locked`: the lock is complete after `pin`, and the bytes are
        // only reproducible if this build changes nothing in it.
        .args([
            "build",
            "--locked",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "-p",
            &member,
        ])
        // Explicit CLI selection wins over inherited CARGO_TARGET_DIR and
        // Cargo configuration, matching the artifact lookup below.
        .arg("--target-dir")
        .arg("target")
        .env("CARGO_ENCODED_RUSTFLAGS", rustflags)
        // the encoded form wins over the plain one, but an inherited
        // `RUSTFLAGS` would be a confusing dead passenger.
        .env_remove("RUSTFLAGS")
        .current_dir(scratch);
    command
}

/// the one place every gated artifact is componentized: the linked
/// componentizer (the crate root), never a binary found on a PATH.
fn componentize(cdylib: &Path, out: &Path) -> Result<(), String> {
    let core = fs::read(cdylib).map_err(|e| format!("reading {}: {e}", cdylib.display()))?;
    let component = guest_builder::componentize(&core)
        .map_err(|e| format!("componentizing {}: {e}", cdylib.display()))?;
    fs::write(out, component).map_err(|e| format!("writing {}: {e}", out.display()))
}

/// an index guest ships as the built cdylib itself — fluentabi is core wasm,
/// so there is nothing to componentize.
fn copy_cdylib(cdylib: &Path, out: &Path) -> Result<(), String> {
    fs::copy(cdylib, out)
        .map(|_| ())
        .map_err(|e| format!("copying {} to {}: {e}", cdylib.display(), out.display()))
}

/// the shell lock, written beside the artifact it describes: the record of what
/// that artifact was built from, and the seed of the next build. `dest` is the
/// module's `guest.lock` for a canonical build and `<out>.lock` for an `--out`
/// one — the same bytes either way, since the shell workspace is the same.
fn write_lock(scratch: &Path, dest: &Path) -> Result<(), String> {
    fs::copy(scratch.join("Cargo.lock"), dest)
        .map(|_| ())
        .map_err(|e| format!("recording the shell lock as {}: {e}", dest.display()))
}

fn cdylib_path(scratch: &Path, name: &str, kind: GuestKind) -> PathBuf {
    scratch
        .join("target/wasm32-unknown-unknown/release")
        .join(format!("{}_{}.wasm", snake(name), kind.member()))
}

// ============================================================================
// small helpers
// ============================================================================

/// the cargo that invoked us (`cargo run` sets `CARGO`), else PATH's.
fn cargo() -> String {
    env::var("CARGO").unwrap_or_else(|_| "cargo".to_string())
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn snake(name: &str) -> String {
    name.replace('-', "_")
}

fn write(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).map_err(|e| format!("writing {}: {e}", path.display()))
}

// ============================================================================
// tests
// ============================================================================

// ---- vendor: the registry a run builds a module against, with no network ----

/// `guest-builder vendor --out <dir> [--directory <path>]`: every crates.io
/// package a module guest build resolves, as a directory plus the cargo config
/// that points at it.
///
/// A run inside a microVM reaches the network through vsock tunnels and
/// nothing else, so a module build there resolves against vendored sources or
/// it does not resolve at all. The set is DERIVED, never listed: one
/// synthesized workspace depending on every crate that declares a `guest`
/// feature, seeded with the checkout's own lockfile, has a lockfile that IS
/// the union of what the modules need — and a crate nobody depends on is
/// simply never in it.
///
/// **The set is the LOCKFILE graph, not the compile graph, and that is not an
/// oversight.** Cargo's resolution is target-independent even though
/// compilation is not: a replaced source must hold every package in the
/// lockfile so the resolver can read its manifest, including ones no wasm32
/// build ever compiles. Filtering by `cargo tree --target
/// wasm32-unknown-unknown` removes `sha2`'s `cpufeatures` — declared under
/// `[target.'cfg(any(target_arch = "x86_64", …))'.dependencies]` — and every
/// build then fails with "no matching package named `cpufeatures` found". It
/// is the same reason `cargo vendor` has no `--target`: it could not honour
/// one. So `aws-lc-sys` rides along at 69 MB, unreachable and required.
///
/// The tree it reads is the platform workspace — the one the command runs in,
/// or `--platform`'s — exactly like a build.
fn vendor(out: &Path, directory: Option<&str>, platform_dir: Option<&Path>) -> Result<(), String> {
    let platform = platform(platform_dir)?;
    let root = platform.root;
    let sdk = module_sdk(&root)?.source(&format!("git = {:?}", platform.git));
    let scratch = root.join("target/guest-builder/vendor-shell");
    let _ = fs::remove_dir_all(&scratch);
    synthesize_vendor_shell(&scratch, &root, &sdk)?;
    // the checkout's OWN pins: a run clones this tree, so the versions it asks
    // for are the versions that have to be on disk. Without the seed cargo
    // resolves to latest-compatible and vendors crates the clone never wants.
    fs::copy(root.join("Cargo.lock"), scratch.join("Cargo.lock"))
        .map_err(|e| format!("seeding the vendor lock: {e}"))?;

    let vendor_dir = out.join("vendor");
    fs::create_dir_all(out).map_err(|e| format!("creating {}: {e}", out.display()))?;
    let output = Command::new(cargo())
        .arg("vendor")
        .arg("--versioned-dirs")
        .arg(&vendor_dir)
        .current_dir(&scratch)
        .output()
        .map_err(|e| format!("running cargo vendor: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo vendor: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // cargo writes the config with the directory it just filled; an image build
    // fills it on the host and mounts it elsewhere, so `--directory` renames it
    // without touching the git-source stanzas, which carry exact revisions and
    // are nobody's to hand-write.
    let config = String::from_utf8_lossy(&output.stdout).into_owned();
    let named = match directory {
        None => config,
        Some(path) => config.replace(
            &format!("directory = \"{}\"", vendor_dir.display()),
            &format!("directory = \"{path}\""),
        ),
    };
    write(&out.join("config.toml"), &named)?;
    eprintln!(
        "guest-builder: vendored {} crates for {} guests",
        fs::read_dir(&vendor_dir).map(Iterator::count).unwrap_or(0),
        guest_packages(&root)?.len()
    );
    Ok(())
}

/// one workspace whose sole member depends on EVERY guest crate in the
/// checkout, carrying the same wasm32 patch set a build shell gets.
fn synthesize_vendor_shell(scratch: &Path, root: &Path, sdk: &str) -> Result<(), String> {
    let shell = scratch.join("shell");
    let src = shell.join("src");
    fs::create_dir_all(&src).map_err(|e| format!("creating {}: {e}", src.display()))?;
    let deps: Vec<String> = guest_packages(root)?
        .into_iter()
        .map(|(name, dir)| {
            format!(
                "{name} = {{ path = {:?}, features = [\"guest\"], default-features = false }}",
                dir.display().to_string()
            )
        })
        .collect();
    write(
        &shell.join("Cargo.toml"),
        &format!(
            "# synthesized by `guest-builder vendor` — do not edit.\n\
             [package]\nname = \"guest-vendor-shell\"\nversion = \"0.0.0\"\n\
             edition = \"2021\"\npublish = false\n\n[lib]\npath = \"src/lib.rs\"\n\n\
             [dependencies]\n{}\n",
            deps.join("\n")
        ),
    )?;
    write(&src.join("lib.rs"), "")?;
    write(
        &scratch.join("Cargo.toml"),
        &format!(
            "# synthesized by `guest-builder vendor` — do not edit.\n\
             [workspace]\nmembers = [\"shell\"]\nresolver = \"2\"\n{}",
            patch_section(sdk)
        ),
    )
}

/// every package in the checkout that declares a `guest` feature — the same
/// question [`read_module`] asks of one directory, asked of the whole tree, so
/// a module added tomorrow is vendored for without anyone editing a list.
fn guest_packages(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let output = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("running cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let meta: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("parsing cargo metadata output: {e}"))?;
    let mut found: Vec<(String, PathBuf)> = meta["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|pkg| {
            pkg["features"]
                .get(GuestKind::Component.feature())
                .is_some()
        })
        .filter_map(|pkg| {
            let name = pkg["name"].as_str()?.to_string();
            let dir = Path::new(pkg["manifest_path"].as_str()?)
                .parent()?
                .to_path_buf();
            Some((name, dir))
        })
        .collect();
    found.sort();
    if found.is_empty() {
        return Err(format!("no guest crates under {}", root.display()));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLATFORM: &str = "https://github.com/ducktape-industries/ducktape";
    const SDK: &str = "https://github.com/ducktape-industries/ducktape-sdk";

    fn sdk_fixture() -> SdkPin {
        sdk_pin_of(&format!("git+{SDK}?branch=dev#abc123")).expect("a lock's git source")
    }

    /// a written git reference is hashed into every symbol name, so the shell
    /// must name the module by source alone and leave the revision to the lock.
    #[test]
    fn the_shell_names_the_module_by_source_alone() {
        let manifest =
            member_manifest("chat", GuestKind::Component, &format!("git = {PLATFORM:?}"));
        assert!(manifest.contains(
            "chat = { git = \"https://github.com/ducktape-industries/ducktape\", default-features = false, features = [\"guest\"] }"
        ));
        assert!(!manifest.contains("rev ="));
        assert!(!manifest.contains("branch ="));
        assert!(manifest.contains("name = \"chat-component\""));
    }

    #[test]
    fn the_workspace_has_one_member_per_declared_guest() {
        let sdk = sdk_fixture();
        let both = workspace_manifest(&[GuestKind::Component, GuestKind::Index], &sdk.source);
        assert!(both.contains("members = [\"component\", \"index\"]"));
        let component_only = workspace_manifest(&[GuestKind::Component], &sdk.source);
        assert!(component_only.contains("members = [\"component\"]"));
    }

    /// the stubs are the module SDK's, and a module's own dependency on the SDK
    /// names the source the way its platform does: spell the patches any other
    /// way and cargo keeps a second checkout, at a second revision.
    #[test]
    fn the_patch_stubs_ride_the_module_sdks_source() {
        let sdk = sdk_fixture();
        assert_eq!(sdk.rev, "abc123");
        let patches = patch_section(&sdk.source);
        assert!(patches.contains(
            "getrandom-02 = { package = \"getrandom\", version = \"0.2\", git = \"https://github.com/ducktape-industries/ducktape-sdk\", branch = \"dev\" }"
        ));
        assert!(patches.contains("blst = { git = \"https://github.com/ducktape-industries/ducktape-sdk\", branch = \"dev\" }"));
        // the revision is the lock's job: a written one would be a second source
        assert!(!patches.contains("rev ="));

        // a platform that pins the SDK by revision is spelled that way instead
        let by_revision = sdk_pin_of(&format!("git+{SDK}#abc123")).expect("source");
        assert_eq!(by_revision.source, format!("git = {SDK:?}"));
        assert_eq!(by_revision.rev, "abc123");
        assert!(sdk_pin_of("registry+https://crates.io").is_err());
    }

    #[test]
    fn a_lock_entry_names_the_source_of_that_package_alone() {
        let lock = "\
[[package]]
name = \"getrandom\"
version = \"0.2.17\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"

[[package]]
name = \"ducktape-module-sdk\"
version = \"0.0.0\"
source = \"git+https://github.com/ducktape-industries/ducktape-sdk?branch=dev#abc123\"
dependencies = [
 \"sdk\",
]
";
        assert_eq!(
            locked_source(lock, MODULE_SDK).as_deref(),
            Some("git+https://github.com/ducktape-industries/ducktape-sdk?branch=dev#abc123")
        );
        assert_eq!(locked_source(lock, "nothing-here"), None);
    }

    /// rustc takes the last matching mapping, and a checkout lives under
    /// CARGO_HOME: every checkout's stable token must come after CARGO_HOME's.
    /// Both repositories get one — a directory named for a revision would
    /// otherwise move a guest's bytes on every commit to either.
    #[test]
    fn every_checkout_mapping_comes_after_cargo_home() {
        let graph = serde_json::json!({
            "packages": [
                {
                    "name": "chat",
                    "source": format!("git+{PLATFORM}#abcdef0"),
                    "manifest_path": "/home/u/.cargo/git/checkouts/ducktape-1234/abcdef0/crates/modules/apps/chat/Cargo.toml",
                },
                {
                    "name": "ducktape-module-sdk",
                    "source": format!("git+{SDK}?branch=dev#9876543"),
                    "manifest_path": "/home/u/.cargo/git/checkouts/ducktape-sdk-5678/9876543/crates/module-sdk/Cargo.toml",
                },
                {
                    "name": "serde",
                    "source": "registry+https://github.com/rust-lang/crates.io-index",
                    "manifest_path": "/home/u/.cargo/registry/src/index.crates.io-1/serde-1.0.0/Cargo.toml",
                },
            ]
        });
        let flags = remap_flags(Path::new("/scratch"), &graph).expect("flags");
        let flags: Vec<&str> = flags.split('\x1f').collect();
        let at = |token: &str| {
            flags
                .iter()
                .position(|flag| flag.ends_with(token))
                .unwrap_or_else(|| panic!("{token} mapping in {flags:?}"))
        };
        assert!(at("=/ducktape") > at("=/cargo"), "{flags:?}");
        assert!(at("=/ducktape-sdk") > at("=/cargo"), "{flags:?}");
        assert_eq!(
            flags[at("=/ducktape")],
            "--remap-path-prefix=/home/u/.cargo/git/checkouts/ducktape-1234/abcdef0=/ducktape"
        );
        assert_eq!(
            flags[at("=/ducktape-sdk")],
            "--remap-path-prefix=/home/u/.cargo/git/checkouts/ducktape-sdk-5678/9876543=/ducktape-sdk"
        );
        // a registry package is not a checkout: nothing to remap but CARGO_HOME
        assert_eq!(flags.len(), 5, "{flags:?}");
    }

    /// the module's own repository, and no repository whose URL merely starts
    /// with it.
    #[test]
    fn platform_inputs_claim_the_platforms_packages_only() {
        let checkout = "/home/u/.cargo/git/checkouts/ducktape-1234/abcdef0";
        let graph = serde_json::json!({
            "packages": [
                {
                    "name": "chat",
                    "source": format!("git+{PLATFORM}#abcdef0"),
                    "manifest_path": format!("{checkout}/crates/modules/apps/chat/Cargo.toml"),
                },
                {
                    "name": "ducktape-module-sdk",
                    "source": format!("git+{SDK}?branch=dev#9876543"),
                    "manifest_path": "/home/u/.cargo/git/checkouts/ducktape-sdk-5678/9876543/crates/module-sdk/Cargo.toml",
                },
            ]
        });
        let inputs = platform_inputs(&graph, Path::new(checkout), PLATFORM).expect("inputs");
        assert!(inputs.contains(Path::new("crates/modules/apps/chat")));
        assert!(inputs.contains(Path::new("Cargo.lock")));
        assert!(
            !inputs
                .iter()
                .any(|path| path.starts_with("crates/module-sdk")),
            "{inputs:?}"
        );
    }

    #[test]
    fn the_checkout_root_is_the_manifest_minus_the_repository_place() {
        let root = checkout_root_of(
            Path::new("/home/u/.cargo/git/checkouts/ducktape-1234/abcdef0/crates/modules/apps/chat/Cargo.toml"),
            Path::new("crates/modules/apps/chat"),
        )
        .expect("root");
        assert_eq!(
            root,
            Path::new("/home/u/.cargo/git/checkouts/ducktape-1234/abcdef0")
        );

        let wrong_place = checkout_root_of(
            Path::new("/somewhere/else/tasks/Cargo.toml"),
            Path::new("crates/modules/apps/chat"),
        );
        assert!(wrong_place.is_err());
    }

    #[test]
    fn build_and_componentization_use_the_same_explicit_target_directory() {
        for root in [
            Path::new("/checkout/target/guest-builder/collaboration"),
            Path::new("relative/scratch"),
        ] {
            let command = build_command(root, "collaboration", GuestKind::Component, "");
            let args: Vec<_> = command.get_args().collect();
            let index = args
                .iter()
                .position(|arg| *arg == "--target-dir")
                .expect("an inherited CARGO_TARGET_DIR must not redirect compilation");
            let target = command.get_current_dir().unwrap().join(args[index + 1]);
            assert_eq!(
                cdylib_path(root, "collaboration", GuestKind::Component),
                target.join("wasm32-unknown-unknown/release/collaboration_component.wasm"),
                "componentization must read the artifact just compiled"
            );
        }
    }

    /// where a round of the redirection test sends cargo's output when the
    /// explicit selection is dropped.
    #[derive(Clone, Copy)]
    enum Redirect {
        Environment,
        Configuration,
    }

    /// An inherited `CARGO_TARGET_DIR` or a `build.target-dir` in
    /// configuration must not move the compiler's output away from the path
    /// the artifact lookup reads: a redirected build either cannot be found at
    /// all, or leaves an EARLIER build's bytes there to be packaged under this
    /// build's lock. Each round compiles a real cdylib through the production
    /// seam and reads back what would be packaged.
    #[test]
    fn a_redirected_cargo_output_cannot_package_an_earlier_build() {
        let work = scratch();
        let shell = work.path().join("shell");
        fixture_file(
            &shell,
            "Cargo.toml",
            "[workspace]\nmembers = [\"component\", \"index\"]\nresolver = \"2\"\n",
        );
        for kind in GuestKind::ALL {
            fixture_file(
                &shell,
                &format!("{}/Cargo.toml", kind.member()),
                &format!(
                    "[package]\nname = \"probe-{}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"cdylib\"]\n",
                    kind.member()
                ),
            );
            fixture_file(&shell, &format!("{}/src/lib.rs", kind.member()), "");
        }
        run_command(
            Command::new(cargo())
                .current_dir(&shell)
                .arg("generate-lockfile"),
        );

        let decoy = |redirect: Redirect| match redirect {
            Redirect::Environment => work.path().join("decoy-environment"),
            Redirect::Configuration => work.path().join("decoy-configuration"),
        };
        let redirect_output = |redirect: Redirect, command: &mut Command| match redirect {
            Redirect::Environment => {
                command.env("CARGO_TARGET_DIR", decoy(redirect));
            }
            Redirect::Configuration => {
                fixture_file(
                    &shell,
                    ".cargo/config.toml",
                    &format!(
                        "[build]\ntarget-dir = {:?}\n",
                        decoy(redirect).display().to_string()
                    ),
                );
            }
        };
        // the same build minus the explicit selection: what the redirection
        // does when nothing overrides it, so a round that redirects nothing
        // cannot pass for one that does.
        let unselected = |kind: GuestKind| {
            let reference = build_command(&shell, "probe", kind, "");
            let mut args: Vec<std::ffi::OsString> =
                reference.get_args().map(ToOwned::to_owned).collect();
            let selection = args
                .iter()
                .position(|arg| arg == "--target-dir")
                .expect("the build must select its target directory explicitly");
            args.drain(selection..selection + 2);
            let mut command = Command::new(cargo());
            command
                .args(args)
                .env("CARGO_ENCODED_RUSTFLAGS", "")
                .env_remove("RUSTFLAGS")
                // "nothing overrides it" has to mean nothing THE OPERATOR
                // brought either. The configuration round redirects with a
                // config file, and cargo ranks the environment above one — so
                // a `CARGO_TARGET_DIR` in the shell running the test wins,
                // the reference build lands in the operator's directory, and
                // the round fails claiming the redirection moved nothing.
                .env_remove("CARGO_TARGET_DIR")
                .current_dir(&shell);
            command
        };

        let mut packaged: std::collections::HashMap<&str, Vec<u8>> = Default::default();
        // round 1 builds a clean scratch; round 2 finds round 1's bytes
        // sitting at the lookup path.
        for (value, redirect) in [(1u32, Redirect::Environment), (2, Redirect::Configuration)] {
            for kind in GuestKind::ALL {
                fixture_file(
                    &shell,
                    &format!("{}/src/lib.rs", kind.member()),
                    &format!("#[no_mangle]\npub extern \"C\" fn value() -> u32 {{ {value} }}\n"),
                );
            }
            let mut without_selection = unselected(GuestKind::Component);
            redirect_output(redirect, &mut without_selection);
            run_command(&mut without_selection);
            assert!(
                decoy(redirect)
                    .join("wasm32-unknown-unknown/release/probe_component.wasm")
                    .is_file(),
                "the redirection under test moved nothing"
            );

            for kind in GuestKind::ALL {
                let mut command = build_command(&shell, "probe", kind, "");
                redirect_output(redirect, &mut command);
                run_command(&mut command);
                let artifact = cdylib_path(&shell, "probe", kind);
                let bytes =
                    fs::read(&artifact).unwrap_or_else(|e| panic!("{}: {e}", artifact.display()));
                assert_ne!(
                    packaged.get(kind.member()),
                    Some(&bytes),
                    "{}: the lookup still holds the earlier build's bytes",
                    kind.member()
                );
                packaged.insert(kind.member(), bytes);
            }
        }
    }

    /// this crate's own tree — where a TEST may look, unlike the tool, which
    /// works in whatever platform workspace it is pointed at.
    fn sdk_repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("bin/guest-builder sits two directories under the repository root")
            .to_path_buf()
    }

    fn scratch() -> tempfile::TempDir {
        let root = sdk_repository().join("target/guest-builder-tests");
        fs::create_dir_all(&root).unwrap();
        tempfile::tempdir_in(root).unwrap()
    }

    fn run_command(command: &mut Command) -> std::process::Output {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = run_command(Command::new("git").arg("-C").arg(repo).args(args));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn fixture_file(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// a second repository standing in for ducktape-sdk: the module SDK and
    /// the wasm32 patch stubs, at TWO revisions — the platform pins the first,
    /// the branch has moved on to the second.
    fn sdk_fixture_repository(root: &Path) -> SdkPin {
        fs::create_dir_all(root).unwrap();
        git(root, &["init", "--initial-branch=base"]);
        git(root, &["config", "user.name", "Guest builder test"]);
        git(
            root,
            &["config", "user.email", "guest-builder@example.invalid"],
        );
        fixture_file(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"module-sdk\"]\nresolver = \"2\"\n",
        );
        fixture_file(
            root,
            "module-sdk/Cargo.toml",
            "[package]\nname = \"ducktape-module-sdk\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        );
        fixture_file(
            root,
            "module-sdk/src/lib.rs",
            "pub const REVISION: u32 = 1;\n",
        );
        for (directory, name, version) in [
            ("random02", "getrandom", "0.2.17"),
            ("random03", "getrandom", "0.3.4"),
            ("random04", "getrandom", "0.4.3"),
            ("blst", "blst", "0.3.16"),
        ] {
            fixture_file(
                root,
                &format!("stubs/{directory}/Cargo.toml"),
                &format!(
                    "[package]\nname = {name:?}\nversion = {version:?}\nedition = \"2021\"\n[workspace]\n"
                ),
            );
            fixture_file(root, &format!("stubs/{directory}/src/lib.rs"), "");
        }
        git(root, &["add", "."]);
        git(
            root,
            &["-c", "commit.gpgsign=false", "commit", "-m", "The SDK"],
        );
        let pinned = git(root, &["rev-parse", "HEAD"]);
        fixture_file(
            root,
            "module-sdk/src/lib.rs",
            "pub const REVISION: u32 = 2;\n",
        );
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "The SDK moves",
            ],
        );
        SdkPin {
            source: format!("git = \"file://{}\"", root.display()),
            rev: pinned,
        }
    }

    fn platform_fixture(root: &Path) -> (Module, String, String, SdkPin) {
        let sdk = sdk_fixture_repository(&root.parent().unwrap().join("sdk"));
        fs::create_dir_all(root).unwrap();
        git(root, &["init", "--initial-branch=base"]);
        git(root, &["config", "user.name", "Guest builder test"]);
        git(
            root,
            &["config", "user.email", "guest-builder@example.invalid"],
        );
        fixture_file(root, ".gitignore", "target/\n");
        fixture_file(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"shared\"]\nresolver = \"2\"\n",
        );
        fixture_file(
            root,
            "shared/Cargo.toml",
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        fixture_file(root, "shared/src/lib.rs", "pub fn value() -> u32 { 1 }\n");
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Base without the new module",
            ],
        );
        // A second source with the same package name forces Cargo to spell
        // source IDs in dependency entries as well as package records.
        let other = root.parent().unwrap().join("other-shared");
        fs::create_dir_all(&other).unwrap();
        git(&other, &["init", "--initial-branch=base"]);
        git(&other, &["config", "user.name", "Guest builder test"]);
        git(
            &other,
            &["config", "user.email", "guest-builder@example.invalid"],
        );
        fixture_file(
            &other,
            "Cargo.toml",
            "[package]\nname = \"shared\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[workspace]\n",
        );
        fixture_file(&other, "src/lib.rs", "pub fn value() -> u32 { 2 }\n");
        git(&other, &["add", "."]);
        git(
            &other,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Independent shared package",
            ],
        );
        git(root, &["switch", "-c", "module"]);
        fixture_file(
            root,
            "Cargo.toml",
            "[workspace]\nmembers = [\"shared\", \"module\"]\nresolver = \"2\"\n",
        );
        fixture_file(
            root,
            "module/Cargo.toml",
            "[package]\nname = \"new-module\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[features]\nguest = []\n[dependencies]\nshared = { path = \"../shared\" }\n",
        );
        let manifest = root.join("module/Cargo.toml");
        let mut content = fs::read_to_string(&manifest).unwrap();
        content.push_str(&format!(
            "other-shared = {{ package = \"shared\", git = \"file://{}\" }}\n",
            other.display()
        ));
        // the module reaches its SDK exactly as a real one does: the second
        // repository, by the source its platform names.
        content.push_str(&format!("ducktape-module-sdk = {{ {} }}\n", sdk.source));
        fs::write(manifest, content).unwrap();
        fixture_file(
            root,
            "module/src/lib.rs",
            "pub fn value() -> u32 { shared::value() }\n",
        );
        git(root, &["add", "."]);
        git(
            root,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Introduce a guest on its branch",
            ],
        );
        let rev = git(root, &["rev-parse", "HEAD"]);
        git(root, &["switch", "base"]);
        let module = Module {
            name: "new-module".into(),
            path: "module".into(),
            guests: vec![GuestKind::Component],
        };
        (module, format!("file://{}", root.display()), rev, sdk)
    }

    /// and pins the module SDK where its platform does, not where the SDK's
    /// own branch has moved on to.
    #[test]
    fn first_build_resolves_a_package_absent_from_the_default_branch() {
        let work = scratch();
        let repo = work.path().join("platform");
        let (module, url, rev, sdk) = platform_fixture(&repo);
        let pinned_sdk = sdk.rev.clone();
        let sdk = ModuleSdk::Pinned(sdk);
        let shell = work.path().join("shell");
        fixture_file(
            &shell,
            "Cargo.lock",
            "stale scratch state must not seed a first build",
        );
        seed_lock(&shell, &repo.join("module")).unwrap();
        assert!(!shell.join("Cargo.lock").exists());
        let graph = pin(&shell, &module, &url, &rev, &sdk).unwrap();
        let checkout = checkout_root(&graph, &module).unwrap();
        assert!(checkout.join("module/src/lib.rs").is_file());
        let lock = fs::read_to_string(shell.join("Cargo.lock")).unwrap();
        assert!(lock.contains(&format!("git+{url}#{rev}")));
        assert!(!lock.contains("?rev="));
        assert_eq!(
            locked_source(&lock, MODULE_SDK)
                .and_then(|source| Some(source.split_once('#')?.1.to_string())),
            Some(pinned_sdk),
            "the guest's SDK is its host's revision, not the branch head"
        );
        assert!(
            !fs::read_to_string(shell.join("component/Cargo.toml"))
                .unwrap()
                .contains("rev =")
        );
        run_command(Command::new(cargo()).current_dir(&shell).args([
            "check",
            "--locked",
            "--target",
            "wasm32-unknown-unknown",
        ]));
        assert_eq!(lock, fs::read_to_string(shell.join("Cargo.lock")).unwrap());
    }

    #[test]
    fn resolved_inputs_refuse_shared_staged_and_untracked_sources() {
        let work = scratch();
        let repo = work.path().join("platform");
        let (module, url, rev, sdk) = platform_fixture(&repo);
        let shell = work.path().join("shell");
        let graph = pin(&shell, &module, &url, &rev, &ModuleSdk::Pinned(sdk)).unwrap();
        let checkout = checkout_root(&graph, &module).unwrap();
        let inputs = platform_inputs(&graph, &checkout, &url).unwrap();
        assert!(inputs.contains(Path::new("shared")));
        git(&repo, &["switch", "module"]);
        refuse_modified_sources(&repo, &inputs).unwrap();
        for path in [
            "module/component.wasm",
            "module/index.wasm",
            "module/guest.lock",
            "unrelated/src/lib.rs",
        ] {
            fixture_file(&repo, path, "not a source in this guest's graph");
        }
        refuse_modified_sources(&repo, &inputs).unwrap();
        fixture_file(&repo, "shared/src/lib.rs", "pub fn value() -> u32 { 2 }\n");
        assert!(
            refuse_modified_sources(&repo, &inputs)
                .unwrap_err()
                .contains("shared/src/lib.rs")
        );
        git(&repo, &["add", "shared/src/lib.rs"]);
        assert!(
            refuse_modified_sources(&repo, &inputs)
                .unwrap_err()
                .contains("shared/src/lib.rs")
        );
        git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "Change shared source",
            ],
        );
        fixture_file(&repo, "shared/src/new.rs", "pub const VALUE: u32 = 3;\n");
        assert!(
            refuse_modified_sources(&repo, &inputs)
                .unwrap_err()
                .contains("shared/src/new.rs")
        );
        fs::remove_file(repo.join("shared/src/new.rs")).unwrap();
        fixture_file(
            &repo,
            ".cargo/config.toml",
            "[build]\nincremental = false\n",
        );
        assert!(
            refuse_modified_sources(&repo, &inputs)
                .unwrap_err()
                .contains(".cargo/config.toml")
        );
    }

    #[test]
    fn both_macros_compile_with_only_the_module_sdk_platform_dependency() {
        let work = scratch();
        let sdk = sdk_repository().join("crates/module-sdk");
        fixture_file(
            work.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"store\", \"snapshot\"]\nresolver = \"2\"\n",
        );
        let implementation = r#"
use ducktape_module_sdk::sdk;
struct Example;
#[async_trait::async_trait(?Send)]
impl sdk::Module for Example {
    fn id(&self) -> sdk::ModuleId { "example".into() }
    fn root(&self) -> sdk::StateRoot { sdk::StateRoot([0; 32]) }
    async fn execute(&mut self, _ctx: &mut dyn sdk::Ctx, _msg: &sdk::Msg) -> Result<(), sdk::Error> { Ok(()) }
}
"#;
        for kind in ["store", "snapshot"] {
            fixture_file(
                work.path(),
                &format!("{kind}/Cargo.toml"),
                &format!(
                    "[package]\nname = \"{kind}-macro-check\"\nversion = \"0.0.0\"\nedition = \"2021\"\n[lib]\ncrate-type = [\"cdylib\"]\n[dependencies]\nducktape-module-sdk = {{ path = {sdk:?} }}\nasync-trait = \"0.1\"\n"
                ),
            );
            let snapshot = match kind {
                "snapshot" => {
                    r#"
impl Example {
    fn snapshot(&self) -> Vec<u8> { Vec::new() }
    fn install(&mut self, _bytes: &[u8], _root: sdk::StateRoot) -> Result<(), sdk::Error> { Ok(()) }
}
"#
                }
                _ => "",
            };
            fixture_file(
                work.path(),
                &format!("{kind}/src/lib.rs"),
                &format!(
                    "{implementation}\n{snapshot}\nducktape_module_sdk::{kind}_guest! {{ id: \"example\", module: Example, shape: ducktape_module_sdk::map_shape(), new: Example }}\n"
                ),
            );
        }
        run_command(Command::new(cargo()).current_dir(work.path()).args([
            "check",
            "--workspace",
            "--target",
            "wasm32-unknown-unknown",
        ]));
    }
}
