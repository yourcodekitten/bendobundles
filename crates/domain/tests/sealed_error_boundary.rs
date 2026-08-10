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
        "crates/humble-client/src/lib.rs",
        "fn net(e: wreq::Error)",
        "SEALING SITE: the crate's only wreq->HumbleError conversion, strips the url (#173)",
    ),
    (
        "crates/humble-client/src/lib.rs",
        "`wreq::Error`'s `Display` **appends th",
        "PROSE: net()'s doc comment, citing the upstream line numbers for the leak",
    ),
    (
        "crates/humble-client/src/lib.rs",
        "the type `wreq::Error` m",
        "PROSE: net()'s doc comment naming the enforcing test",
    ),
    (
        "crates/humble-client/src/lib.rs",
        "so a raw `wreq::Er",
        "PROSE: fn send()'s doc comment",
    ),
    (
        "crates/humble-client/src/lib.rs",
        "also yields a `wreq::Error` — client CONSTRUCTION",
        "PROSE: the .build() comment — client construction mints one and is NOT a request verb",
    ),
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

/// Workspace members, read from the root `Cargo.toml`'s `members` list.
///
/// **NOT a `crates/*` directory glob, and that distinction is a gate finding (OMBB, 2026-08-10).**
/// The scan set was a DIRECTORY while the workspace is a MANIFEST LIST: a member declared outside
/// `crates/` was invisible to the census **and the old `>= 7` floor still passed**, so the
/// fail-closed guard did not fire either. Membership is declared in exactly one place; read it there.
fn workspace_members() -> Vec<PathBuf> {
    let root = workspace_root();
    let manifest = root.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap_or_else(|e| panic!("FAIL CLOSED: cannot read {}: {e}", manifest.display()));
    let start = text
        .find("members")
        .expect("FAIL CLOSED: root Cargo.toml has no `members` key");
    let open = text[start..]
        .find('[')
        .expect("FAIL CLOSED: `members` has no opening bracket");
    let close = text[start + open..]
        .find(']')
        .expect("FAIL CLOSED: `members` has no closing bracket");
    let members: Vec<PathBuf> = text[start + open + 1..start + open + close]
        .split(',')
        .map(|s| s.trim().trim_matches('"').trim())
        .filter(|s| !s.is_empty())
        .map(|s| root.join(s))
        .collect();
    assert!(
        members.len() >= 7,
        "FAIL CLOSED: parsed only {} workspace members from {} — the parser is under-reporting and \
         every verdict below would be silently scoped to a subset. Parsed: {members:?}",
        members.len(),
        manifest.display()
    );
    for m in &members {
        assert!(
            m.join("Cargo.toml").is_file(),
            "FAIL CLOSED: member {} has no Cargo.toml — `members` and the tree disagree",
            m.display()
        );
    }
    members
}

/// Every `.rs` file under every workspace member's `src/` **and `tests/`**.
///
/// `tests/` is included because this file's header claims a **workspace-wide** invariant while 8 test
/// files were unscanned. **A claim wider than its instrument is the thing this file exists to stop**
/// (OMBB, 2026-08-10). `src/bin/` arrives via the recursive walk — which is how
/// `humble-client/src/bin/probe.rs` reaches this census while `cargo check` never compiles it
/// (`required-features = ["probe"]`): *invisible to the compiler, fully visible here.*
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
    let mut out = Vec::new();
    for member in workspace_members() {
        for sub in ["src", "tests"] {
            let d = member.join(sub);
            if d.is_dir() {
                walk(&d, &mut out);
            }
        }
    }
    out.sort();
    // FAIL CLOSED: an empty or implausibly small census is indistinguishable from a passing one.
    assert!(
        out.len() >= 15,
        "FAIL CLOSED: found only {} rust sources across members' src/ and tests/ — expected at \
         least one src per member and several tests. A census that found nothing must not pass.",
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
///
/// 🔴 **THIS CONTROL USED TO BE ANCHORED IN THE EASIEST PLACE IN THE TREE, AND WAS THEREFORE USELESS**
/// (OMBB, 2026-08-10). It grepped `OperatorMessage`, which lives in
/// `crates/fulfillment/src/operator_message.rs` — the most findable file in the workspace. But the
/// scanner's realistic failure is **a directory it never reaches**, and a control sitting where
/// success is easy cannot detect that. `humble-client/src/bin/probe.rs` sat outside the old scan and
/// needed a human reviewer to find; **had this control been anchored in a `src/bin/`, the control
/// would have failed instead of the reviewer having to.**
///
/// ⇒ ***A LABELLED POSITIVE PLACED WHERE SUCCESS IS EASY CANNOT DETECT THE FAILURE MODE YOU ACTUALLY
/// HAVE.*** So this asserts **reachability of the hard directories** by path, which cannot drift the
/// way a string sentinel can — and keeps the string check too, since a path can be reached while the
/// reader is broken.
#[test]
fn the_scanner_reaches_the_directories_it_is_most_likely_to_miss() {
    let root = workspace_root();
    let rel: Vec<String> = rust_sources()
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    for (needle, why) in [
        (
            "/src/bin/",
            "a bin target — probe.rs builds its own client and hides behind required-features, so \
             the compiler never sees it and only this census can",
        ),
        (
            "/tests/",
            "integration tests — 8 files were unscanned while this file claimed workspace-wide",
        ),
    ] {
        assert!(
            rel.iter().any(|p| p.contains(needle)),
            "FAIL CLOSED: the scanner reached ZERO files under `{needle}` ({why}). Every \
             'no occurrences' verdict here is scoped to whatever it did reach, which is not what \
             the header claims. Scanned {} files.",
            rel.len()
        );
    }
    assert!(
        !occurrences_of("OperatorMessage").is_empty(),
        "FAIL CLOSED: zero occurrences of `OperatorMessage`, which exists. The reader is broken."
    );
}

/// NEGATIVE CONTROL. A detector with no labelled negative cannot be shown to discriminate.
/// The sentinel is **assembled at runtime and never appears as a literal**, because this file is
/// itself in the scan set now that `tests/` is walked. Written as one literal it matches itself and
/// the control fails for a reason unrelated to the scanner — *observed, not theorised: that is
/// exactly what happened the first time this ran after `tests/` was added.*
#[test]
fn the_scanner_reports_nothing_for_a_string_that_is_definitely_absent() {
    let sentinel = format!("ZZZ_{}_{}_ZZZ", "NEGATIVE", "CONTROL_SEALED_BOUNDARY");
    let hits = occurrences_of(&sentinel);
    assert!(
        hits.is_empty(),
        "the scanner reported hits for a sentinel that appears nowhere: {hits:?}"
    );
}

/// The one file where naming a URL-bearing type is definitionally required: this one. `URL_BEARING`
/// and the allow-list snippets cannot do their job without spelling the type names out.
///
/// ⚠️ **THIS IS AN EXEMPTION, NOT A NARROWING, AND THE DIFFERENCE IS THE POINT.** When `tests/` came
/// into scope the tempting fix was to drop this file from `rust_sources()` — *"narrow the scan until
/// it passes"*, the move the gate explicitly warned against. Instead the file stays fully scanned by
/// every other test here, and only the **name** census skips it, for a stated reason, with a control
/// proving the exemption covers exactly one path.
const CENSUS_SELF: &str = "crates/domain/tests/sealed_error_boundary.rs";

/// The self-exemption must cover exactly one real file. An exemption that widens — or that points at
/// a path which no longer exists — is a hole with a comment on it.
#[test]
fn the_self_exemption_covers_exactly_one_file() {
    let root = workspace_root();
    let n = rust_sources()
        .iter()
        .filter(|p| {
            p.strip_prefix(&root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/")
                == CENSUS_SELF
        })
        .count();
    assert_eq!(
        n, 1,
        "FAIL CLOSED: the name census's self-exemption matched {n} files, not 1 (`{CENSUS_SELF}`)."
    );
}

#[test]
fn url_bearing_error_types_are_named_only_where_they_are_sealed() {
    let mut unreviewed = Vec::new();
    for needle in URL_BEARING {
        for (file, line, content) in occurrences_of(needle) {
            if file == CENSUS_SELF {
                continue; // see CENSUS_SELF — stated exemption, proven single-file above
            }
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
