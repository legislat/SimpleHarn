//! Build-time release identity.
//!
//! `version` stays the crate semver; the release identity is a separate fact
//! resolved once at build time and handed to the compiler as
//! `SIMPLEHARN_RELEASE`: `/har/status` reports both and the boot banner logs
//! both. A build from a tagged clean checkout embeds its exact tag; an
//! untagged build, a dirty tree, or a machine without git embeds the empty
//! value, which means "no release identity". This script never fails the
//! build: a checkout that cannot answer the question is a legitimate way to
//! build, and the answer then is simply "none".

use std::process::Command;

/// Grammar of a release identity: `x.y.z` or `x.y.z.h`, digits only — the tag
/// rule of docs/process/GIT_WORKFLOW.md, "Tag and version naming".
fn valid_release(tag: &str) -> bool {
    let mut parts = tag.split('.');
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(a), Some(b), Some(c), None, None) => numeric(a) && numeric(b) && numeric(c),
        (Some(a), Some(b), Some(c), Some(d), None) => {
            numeric(a) && numeric(b) && numeric(c) && numeric(d)
        }
        _ => false,
    }
}

/// Trim, strip one optional leading `v`, and keep only the grammar above.
fn normalize(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let body = trimmed.strip_prefix('v').unwrap_or(trimmed);
    valid_release(body).then(|| body.to_owned())
}

/// Run git in the crate root and answer with its trimmed stdout. `None` when
/// git is absent, exits non-zero, or prints non-UTF-8 — a build without a
/// usable git is still a build.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|stdout| stdout.trim().to_owned())
}

/// The tag that names exactly this commit, and only from a tracked tree that
/// is clean: a build carrying local changes is not the tagged build, so it
/// claims no release identity.
fn exact_clean_tag() -> Option<String> {
    let status = git(&["status", "--porcelain", "--untracked-files=no"])?;
    if !status.is_empty() {
        return None;
    }
    normalize(&git(&[
        "describe",
        "--tags",
        "--exact-match",
        "--match",
        "v*",
    ])?)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A tag or branch change rewrites one of these; without them an
    // incremental build keeps the identity of the previous checkout.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/packed-refs");
    println!("cargo:rerun-if-changed=.git/refs/tags");
    println!("cargo:rerun-if-env-changed=SIMPLEHARN_RELEASE_TAG");
    // The operator override stands first so a release build can name its tag
    // without a clean checkout. An override that does not parse falls through
    // to the checkout rather than failing the build.
    let override_tag = std::env::var("SIMPLEHARN_RELEASE_TAG")
        .ok()
        .and_then(|raw| normalize(&raw));
    let release = override_tag.or_else(exact_clean_tag).unwrap_or_default();
    // Always emit, empty included: the code reads the empty value as "no
    // release identity", so an unset constant can never be mistaken for one.
    println!("cargo:rustc-env=SIMPLEHARN_RELEASE={release}");
}
