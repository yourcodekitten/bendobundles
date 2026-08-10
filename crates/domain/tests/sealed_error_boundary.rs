//! WORKSPACE INVARIANT — a URL-bearing foreign error type may be NAMED only where it is sealed.
//!
//! Doctrine: `crates/fulfillment/src/operator_message.rs` module header. Origin: #173, where this
//! class was found for the third time, having been hand-fixed twice.
//!
//! WHY THIS LIVES IN `domain`: it is a workspace-wide invariant, and `domain` has three direct
//! dependencies, so this binary links where a `fulfillment` test binary will not. Measured
//! 2026-08-10: `cargo test -p domain --no-run -j 1` peaks at 317,548 kB in 26.8 s, which makes the
//! sabotage controls a local loop instead of four CI round-trips. A control that is expensive to run
//! is a control that gets skipped.
//!
//! BLIND SPOT, STATED OUT LOUD: this checker is syntactic. It cannot see a URL-bearing error type
//! that arrives under another name because a direct dependency RE-EXPORTS it (`dep::TheirError`).
//! Closing that needs type resolution, which this deliberately does not attempt. The
//! dependency-verdict test is the compensating control: a human gives every direct dependency a
//! verdict, and that verdict is a claim about its re-exports too.
//!
//! NOR does a name census see a value bound by INFERENCE — `.send().await` yields a
//! `reqwest::Error` with that name nowhere on the line. That is what the verb-count test covers.

use std::fs;
use std::path::{Path, PathBuf};

/// Foreign crates whose error type renders the request URL.
/// `wreq` 5.3.0 is a `reqwest` lineage fork: Display appends the URL (error.rs:229-230), Debug
/// prints the `url` field (error.rs:198-199), `without_url()` exists (error.rs:77).
const URL_BEARING: &[&str] = &["wreq::Error", "reqwest::Error"];

/// Every reviewed occurrence of a `URL_BEARING` path: (workspace-relative file, snippet the line
/// must contain, why it is allowed).
///
/// PROSE COUNTS, AND THAT IS DELIBERATE. The checker does not strip comments: a comment-stripper's
/// failure mode is a false negative, and a census that fails silently is the defect this file
/// exists to remove. Five lines of cost, and the list doubles as an index of every place in the
/// workspace where this class is reasoned about.
const REVIEWED_OCCURRENCES: &[(&str, &str, &str)] = &[
    (
        "crates/steam-client/src/lib.rs",
        "fn net(e: reqwest::Error)",
        "SEALING SITE: strips the url before stringifying",
    ),
    (
        "crates/steam-client/src/lib.rs",
        "reqwest::Error::Display can include",
        "PROSE: names the leak at the sealing site",
    ),
    (
        "crates/fulfillment/src/operator_message.rs",
        "on a `reqwest::Error`",
        "PROSE: the doctrine header this invariant implements",
    ),
    (
        "crates/fulfillment/src/lib.rs",
        "`reqwest::Error`'s `Display` APPENDS",
        "PROSE: #171 gate comment at deliver(), the crate's only reqwest error boundary",
    ),
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/domain`.
    let d = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = d
        .parent()
        .and_then(Path::parent)
        .expect("manifest dir has two ancestors")
        .to_path_buf();
    assert!(
        root.join("Cargo.toml").is_file() && root.join("crates").is_dir(),
        "FAIL CLOSED: {} does not look like the workspace root — the census would scan nothing \
         and pass vacuously",
        root.display()
    );
    root
}

fn rust_sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("FAIL CLOSED: cannot read {}: {e}", dir.display()));
        for e in entries {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let root = workspace_root();
    let mut out = Vec::new();
    for crate_dir in fs::read_dir(root.join("crates")).expect("crates/ readable") {
        let src = crate_dir.expect("entry").path().join("src");
        if src.is_dir() {
            walk(&src, &mut out);
        }
    }
    out.sort();
    // FAIL CLOSED: an empty or implausibly small census is indistinguishable from a passing one.
    assert!(
        out.len() >= 7,
        "FAIL CLOSED: found only {} rust sources under crates/*/src — expected at least one per \
         crate. A census that found nothing must not report success.",
        out.len()
    );
    out
}

/// (workspace-relative path, 1-based line, line content) for every line containing `needle`.
fn occurrences_of(needle: &str) -> Vec<(String, usize, String)> {
    let root = workspace_root();
    let mut hits = Vec::new();
    for path in rust_sources() {
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("FAIL CLOSED: cannot read {}: {e}", path.display()));
        let rel = path
            .strip_prefix(&root)
            .expect("source under root")
            .to_string_lossy()
            .replace('\\', "/");
        for (i, line) in text.lines().enumerate() {
            if line.contains(needle) {
                hits.push((rel.clone(), i + 1, line.trim().to_string()));
            }
        }
    }
    hits
}

/// POSITIVE CONTROL. Absence is only a measurement if the instrument could have seen the thing.
/// If this fails, every "no occurrences" verdict in this file is void.
#[test]
fn the_scanner_can_see_a_string_that_is_definitely_there() {
    let hits = occurrences_of("OperatorMessage");
    assert!(
        !hits.is_empty(),
        "FAIL CLOSED: the scanner found zero occurrences of `OperatorMessage`, which exists in \
         crates/fulfillment/src/operator_message.rs. The scanner is broken, so no other assertion \
         in this file means anything."
    );
}

/// NEGATIVE CONTROL. A detector with no labelled negative cannot be shown to discriminate.
#[test]
fn the_scanner_reports_nothing_for_a_string_that_is_definitely_absent() {
    let hits = occurrences_of("ZZZ_NEGATIVE_CONTROL_SEALED_BOUNDARY_ZZZ");
    assert!(
        hits.is_empty(),
        "the scanner reported hits for a sentinel that appears nowhere: {hits:?}"
    );
}

#[test]
fn url_bearing_error_types_are_named_only_where_they_are_sealed() {
    let mut unreviewed = Vec::new();
    for needle in URL_BEARING {
        for (file, line, content) in occurrences_of(needle) {
            let reviewed = REVIEWED_OCCURRENCES
                .iter()
                .any(|(f, snippet, _)| *f == file && content.contains(snippet));
            if !reviewed {
                unreviewed.push(format!("  {file}:{line}\n      {content}"));
            }
        }
    }
    assert!(
        unreviewed.is_empty(),
        "A URL-bearing client error type is named at {} unreviewed site(s).\n\n{}\n\n\
         These types render the request URL (Display appends it; Debug prints the `url` field), so \
         a value of one must not be stored in a workspace error type or logged. Either seal it \
         (see `fn net` in crates/steam-client/src/lib.rs) or, if the occurrence is prose or a new \
         sealing site, add it to REVIEWED_OCCURRENCES in this file with a reason.",
        unreviewed.len(),
        unreviewed.join("\n")
    );
}

/// The allow-list must not rot in the other direction either: an entry matching nothing is a stale
/// claim, and a stale allow-list silently widens what the census permits.
#[test]
fn every_reviewed_occurrence_still_matches_a_real_line() {
    let mut stale = Vec::new();
    for (file, snippet, reason) in REVIEWED_OCCURRENCES {
        let found = URL_BEARING.iter().any(|needle| {
            occurrences_of(needle)
                .iter()
                .any(|(f, _, content)| f == file && content.contains(snippet))
        });
        if !found {
            stale.push(format!("  {file} :: {snippet:?} ({reason})"));
        }
    }
    assert!(
        stale.is_empty(),
        "REVIEWED_OCCURRENCES has {} entry(ies) matching no line in the tree:\n{}\n\n\
         The code moved and the allow-list did not. Remove the entry or update its snippet.",
        stale.len(),
        stale.join("\n")
    );
}
