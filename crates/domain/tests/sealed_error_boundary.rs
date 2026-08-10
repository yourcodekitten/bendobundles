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

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// THE STALENESS HALF — every direct dependency carries a reviewed verdict, and the verdict must be
// SIGNED for re-exports.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Verdicts for every DIRECT dependency of every workspace member.
///
/// **Population argument.** A workspace error enum can only STORE a type it can NAME, and it can only
/// name a type from a direct dependency. Measured 2026-08-10: the workspace has **zero `anyhow`, zero
/// `eyre`, zero `Box<dyn Error>`** — no erasure hatch through which a transitive crate's error could
/// arrive unnamed. So "review every direct dependency" is exhaustive **for the storage half**, not a
/// heuristic hoping to spot clients. It says nothing about a value bound by inference.
///
/// **THE THIRD FIELD IS THE RE-EXPORT SIGNATURE, AND IT IS A FIELD RATHER THAN A CAVEAT ON PURPOSE.**
/// This checker is syntactic, so it cannot see a foreign error type arriving under another name
/// because a direct dependency re-exports it (`dep::TheirError`). That gap was going to live in this
/// module's doc comment until Lilith pointed out the obvious: **a caveat in a test's doc comment is
/// the worst available location, because the GREEN is what gets read and the comment is furniture.**
/// So it is a value the same test enforces. **A blind spot that ships becomes a row somebody signed.**
const DEP_VERDICTS: &[(&str, &str, &str)] = &[
    ("argon2", "ReviewedSafe", "re-exports checked: none"),
    (
        "async-trait",
        "NoErrorTypeHeld",
        "re-exports checked: none (proc-macro)",
    ),
    (
        "aws-config",
        "ReviewedSafe",
        "re-exports checked: aws-smithy types; captured as a String at dynamo/src/lib.rs:310,338 via format!(\"{sdk_err:?}\") — 16 sites, see #151",
    ),
    (
        "aws-sdk-dynamodb",
        "ReviewedSafe",
        "re-exports checked: aws-smithy SdkError; same 16 capture sites, see #151",
    ),
    (
        "aws-sdk-lambda",
        "ReviewedSafe",
        "re-exports checked: aws-smithy SdkError",
    ),
    (
        "aws-sdk-ssm",
        "ReviewedSafe",
        "re-exports checked: aws-smithy SdkError",
    ),
    (
        "axum",
        "ReviewedSafe",
        "re-exports checked: http/hyper error types, none stored by us",
    ),
    ("data-encoding", "ReviewedSafe", "re-exports checked: none"),
    ("hmac", "NoErrorTypeHeld", "re-exports checked: none"),
    (
        "lambda_http",
        "ReviewedSafe",
        "re-exports checked: http types; adapter errors not stored (see #186)",
    ),
    (
        "lambda_runtime",
        "ReviewedSafe",
        "re-exports checked: as lambda_http",
    ),
    (
        "reqwest",
        "UrlBearing",
        "sealed at steam-client fn net + keyed_json, and fulfillment deliver()",
    ),
    ("serde", "ReviewedSafe", "re-exports checked: none"),
    (
        "serde_json",
        "ReviewedSafe",
        "re-exports checked: none; stored as Parse(serde_json::Error)",
    ),
    ("sha1", "NoErrorTypeHeld", "re-exports checked: none"),
    ("sha2", "NoErrorTypeHeld", "re-exports checked: none"),
    (
        "thiserror",
        "NoErrorTypeHeld",
        "re-exports checked: none (derive only)",
    ),
    ("time", "ReviewedSafe", "re-exports checked: none"),
    (
        "tokio",
        "ReviewedSafe",
        "re-exports checked: io::Error, no URL surface",
    ),
    ("tracing", "NoErrorTypeHeld", "re-exports checked: none"),
    (
        "tracing-subscriber",
        "ReviewedSafe",
        "re-exports checked: none stored",
    ),
    ("urlencoding", "ReviewedSafe", "re-exports checked: none"),
    ("uuid", "ReviewedSafe", "re-exports checked: none"),
    (
        "wreq",
        "UrlBearing",
        "sealed at humble-client fn net (#173)",
    ),
    (
        "wreq-util",
        "ReviewedSafe",
        "re-exports checked: wreq types — emulation/profile config only, no error stored",
    ),
];

/// Parse the `[dependencies]` section of a Cargo.toml.
///
/// **TWO SYNTAXES, AND MISSING ONE IS A SILENT UNDERCOUNT.** This workspace writes both
/// `serde.workspace = true` and `tokio = { version = "1", features = [...] }`. A pattern anchored on
/// `^name =` sees only the second and reports `domain` as having **NO dependencies at all** — which is
/// how this parser was first written on 2026-08-10, and it under-reported *every* member while looking
/// correct. `[dev-dependencies]` is excluded: the property is about production error types.
fn direct_deps(manifest: &Path) -> Vec<String> {
    let text = fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("FAIL CLOSED: cannot read {}: {e}", manifest.display()));
    let mut in_deps = false;
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]";
            continue;
        }
        if !in_deps || t.is_empty() || t.starts_with('#') {
            continue;
        }
        let name: String = t
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

/// CONTROL for the parser above: it must see a dependency written in EACH syntax. Without this, a
/// parser that silently sees one form passes every verdict test vacuously.
#[test]
fn the_manifest_parser_sees_both_dependency_syntaxes() {
    let root = workspace_root();
    let d = direct_deps(&root.join("crates/domain/Cargo.toml"));
    assert!(
        d.contains(&"thiserror".to_string()),
        "FAIL CLOSED: parser missed `thiserror.workspace = true` in crates/domain/Cargo.toml — it \
         found {d:?}. Every verdict below would be vacuous."
    );
    let f = direct_deps(&root.join("crates/fulfillment/Cargo.toml"));
    assert!(
        f.contains(&"tokio".to_string()),
        "FAIL CLOSED: parser missed a table-syntax dependency in crates/fulfillment/Cargo.toml — it \
         found {f:?}."
    );
}

#[test]
fn every_direct_dependency_has_a_reviewed_verdict() {
    let members = workspace_members();
    let member_names: Vec<String> = members
        .iter()
        .map(|m| {
            m.file_name()
                .expect("member has a directory name")
                .to_string_lossy()
                .to_string()
        })
        .collect();
    let mut found: Vec<String> = Vec::new();
    for m in &members {
        found.extend(direct_deps(&m.join("Cargo.toml")));
    }
    found.retain(|d| !member_names.contains(d));
    found.sort();
    found.dedup();
    assert!(
        found.len() >= 20,
        "FAIL CLOSED: only {} foreign direct dependencies parsed — expected ~25. The parser is \
         under-reporting and this test would pass vacuously. Found: {found:?}",
        found.len()
    );

    let reviewed: Vec<&str> = DEP_VERDICTS.iter().map(|(n, _, _)| *n).collect();
    let unreviewed: Vec<&String> = found
        .iter()
        .filter(|d| !reviewed.contains(&d.as_str()))
        .collect();
    let vanished: Vec<&&str> = reviewed
        .iter()
        .filter(|r| !found.iter().any(|f| f == *r))
        .collect();

    assert!(
        unreviewed.is_empty(),
        "New direct dependency(ies) with no verdict: {unreviewed:?}\n\n\
         Decide, for each: can its error type render a request URL or any credential? If yes, mark it \
         UrlBearing and seal it at its boundary the way crates/humble-client's `fn net` does. If no, \
         mark it ReviewedSafe — and note that verdict also claims its RE-EXPORTS are safe, which is \
         this census's one stated blind spot. Then add it to DEP_VERDICTS."
    );
    assert!(
        vanished.is_empty(),
        "DEP_VERDICTS reviews dependency(ies) the workspace no longer has: {vanished:?}\n\n\
         Remove them. A verdict table that rots in this direction quietly grows the set of names that \
         would pass review without anyone looking."
    );
}

/// The re-export signature is required, not decorative. An unsigned row is an unreviewed row.
#[test]
fn every_verdict_carries_a_re_export_signature() {
    let unsigned: Vec<&str> = DEP_VERDICTS
        .iter()
        .filter(|(_, _, sig)| !sig.contains("re-exports checked") && !sig.contains("sealed at"))
        .map(|(n, _, _)| *n)
        .collect();
    assert!(
        unsigned.is_empty(),
        "Dependency verdict(s) with no re-export signature: {unsigned:?}\n\n\
         This checker is syntactic: it cannot see a foreign error type re-exported under another name. \
         The verdict IS the compensating control, so it must state that the re-exports were looked at. \
         Write `re-exports checked: <what you found>`, or for a UrlBearing crate `sealed at <where>`."
    );
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// THE MINT-SITE CENSUS — scoped by the DEPENDENCY GRAPH, and membership matters more than counts.
// ─────────────────────────────────────────────────────────────────────────────────────────────────

/// Calls that can bring a raw client error into existence.
///
/// ⚠️ **THESE SPELLINGS ARE AN OPEN SET AND THE FILE HAS TO SAY SO** (OMBB, 2026-08-10). `.json(`
/// alone misses `steam-client`'s `.json::<T>()` — the turbofish — so both are listed; and a census
/// keyed on **call syntax** has no closed set of forms. Tomorrow it is `let v: T = r.json().await?`,
/// or a trait method someone re-exports. **Two needles are a patch on an open set; this sentence is
/// what stops the third being a surprise.** The enforcement is the compiler (no `From` impl to
/// auto-convert through); this census only notices change.
///
/// **Comments are NOT stripped, and that is measured, not lazy.** Stripping from `//` to end-of-line
/// looks safe until a string literal contains `//` — this repo is full of `"https://..."`, so a naive
/// strip truncates the literal and can lose a real call to its right. **A false negative in a census
/// is the failure that never announces itself**, so prose is counted and its share is stated per row.
const MINTING_VERBS: &[&str] = &[".send()", ".bytes()", ".text()", ".json(", ".json::<"];

/// Source with **whole-line comments removed**, for the verb census only.
///
/// 🔴 **WHY THIS EXISTS: THE DOC COMMENT THAT DOCUMENTS THE INVARIANT BROKE THE COUNT THAT ENFORCES
/// IT.** `humble-client/src/lib.rs` contains the literal `.send()` four times — once as code, three
/// times in the prose that says *"The only `.send()` in this crate"*. Counting raw occurrences is not
/// merely noisy: **a real new call can be offset by deleting a prose mention, holding the total
/// constant — a FALSE NEGATIVE, not a false alarm.** (OMBB, 2026-08-10; third instance of prose
/// colliding with this census after the doctrine paragraph and `probe.rs`.)
///
/// **Only lines whose trimmed form STARTS with `//` are dropped, never a `//` mid-line.** Cutting
/// from a mid-line `//` would truncate string literals — this repo is full of `"https://..."` — and
/// could lose a real call to the right of one. **Residual, stated: a trailing comment mentioning a
/// verb is still counted. That is a false POSITIVE, which fails in the safe direction.**
fn code_only(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pinned per-FILE verb counts for the `src/` of members that can mint one.
/// `(workspace-relative file, verb, occurrences, note)`
const REVIEWED_VERB_COUNTS: &[(&str, &str, usize, &str)] = &[
    (
        "crates/humble-client/src/lib.rs",
        ".send()",
        1,
        "fn send — the sealer, and the crate's only one",
    ),
    (
        "crates/humble-client/src/lib.rs",
        ".bytes()",
        1,
        "fn body — the sealer, and the crate's only one",
    ),
    (
        "crates/humble-client/src/bin/probe.rs",
        ".send()",
        1,
        "probe builds its OWN wreq::Client; required-features=[\"probe\"] so the compiler never sees this file — only this census does",
    ),
    (
        "crates/humble-client/src/bin/probe.rs",
        ".bytes()",
        1,
        "same: outside the lib's sealing helpers by construction",
    ),
    (
        "crates/steam-client/src/lib.rs",
        ".send()",
        10,
        "UNFORCED — fn net exists and nothing requires it; pinned at today's debt, see #187",
    ),
    (
        "crates/steam-client/src/lib.rs",
        ".bytes()",
        1,
        "UNFORCED — see #187",
    ),
    (
        "crates/steam-client/src/lib.rs",
        ".text()",
        1,
        "UNFORCED — see #187",
    ),
    (
        "crates/steam-client/src/lib.rs",
        ".json::<",
        1,
        "keyed_json's Parse path, sealed with without_url(); the needle `.json(` would miss this turbofish",
    ),
    (
        "crates/fulfillment/src/lib.rs",
        ".send()",
        2,
        "deliver() — the crate's only reqwest error boundary, sealed at #171 — plus a test helper",
    ),
    (
        "crates/fulfillment/src/lib.rs",
        ".json(",
        1,
        "deliver()'s request body; not an error-rendering path",
    ),
    (
        "crates/fulfillment/src/main.rs",
        ".send()",
        2,
        "client construction in the lambda entrypoint",
    ),
];

/// Members that can mint a raw client error, **derived from the dependency graph rather than listed**.
///
/// A member with no `UrlBearing` direct dependency is **structurally incapable** of producing one.
/// Measured 2026-08-10: `.send()` is also the AWS SDK's operation-builder method, so a workspace-wide
/// count is dominated by calls that cannot fail this way — `dynamo` alone has **73**. Deriving the
/// scope deletes 82 such occurrences **mechanically, with zero judgment calls and nothing to
/// maintain** — the same discipline as reading the scan set from `members` instead of globbing
/// `crates/*`. (OMBB's, and it is strictly better than the hand-written file list I proposed.)
fn url_bearing_members() -> Vec<PathBuf> {
    let url_bearing: Vec<&str> = DEP_VERDICTS
        .iter()
        .filter(|(_, verdict, _)| *verdict == "UrlBearing")
        .map(|(n, _, _)| *n)
        .collect();
    assert!(
        !url_bearing.is_empty(),
        "FAIL CLOSED: no UrlBearing crates in DEP_VERDICTS — the scope would be empty and every \
         verb assertion below would pass vacuously"
    );
    let out: Vec<PathBuf> = workspace_members()
        .into_iter()
        .filter(|m| {
            let deps = direct_deps(&m.join("Cargo.toml"));
            deps.iter().any(|d| url_bearing.contains(&d.as_str()))
        })
        .collect();
    assert!(
        !out.is_empty(),
        "FAIL CLOSED: no member declares a UrlBearing dependency — scope empty, assertions vacuous"
    );
    out
}

/// Files under those members' `src/` that contain at least one minting verb.
/// `tests/` is excluded here on purpose: a test hitting a mock server mints nothing that ships. It
/// stays in the NAME census, where a test *naming* `wreq::Error` is a real occurrence of the class.
fn files_with_client_verbs() -> Vec<String> {
    let root = workspace_root();
    let members = url_bearing_members();
    let mut out: Vec<String> = rust_sources()
        .into_iter()
        .filter(|p| members.iter().any(|m| p.starts_with(m.join("src"))))
        .filter(|p| {
            let text = fs::read_to_string(p).expect("readable");
            let code = code_only(&text);
            MINTING_VERBS.iter().any(|v| code.contains(v))
        })
        .map(|p| {
            p.strip_prefix(&root)
                .expect("under root")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// 🔑 **MEMBERSHIP — the durable half.** Counts catch a change in a file you already know about;
/// **membership catches the file nobody told you about.** Declaring the unit documents where the seam
/// is; it does not close it — a pinned list of three files cannot notice a fourth client file added
/// next month. (OMBB, 2026-08-10, correcting a "declare the unit" proposal that would have repeated
/// the same scope seam one level down.)
#[test]
fn the_set_of_files_with_client_verbs_is_exactly_the_reviewed_set() {
    let actual = files_with_client_verbs();
    let mut pinned: Vec<String> = REVIEWED_VERB_COUNTS
        .iter()
        .map(|(f, _, _, _)| (*f).to_string())
        .collect();
    pinned.sort();
    pinned.dedup();
    let missing: Vec<&String> = pinned.iter().filter(|f| !actual.contains(f)).collect();
    let unreviewed: Vec<&String> = actual.iter().filter(|f| !pinned.contains(f)).collect();
    assert!(
        unreviewed.is_empty(),
        "A file in a client-dependent crate contains a minting verb and is NOT in \
         REVIEWED_VERB_COUNTS: {unreviewed:?}\n\n\
         Someone added a network call in a crate that can mint a raw client error. Route it through \
         that crate's sealer (see `fn net`/`fn send` in crates/humble-client/src/lib.rs) and pin the \
         file's counts here, in the same commit."
    );
    assert!(
        missing.is_empty(),
        "REVIEWED_VERB_COUNTS pins file(s) that no longer contain any minting verb: {missing:?}\n\n\
         Remove the rows. A pin list that rots in this direction quietly shrinks what is watched."
    );
}

/// COUNTS — the weak half, kept because a count moving inside a known file is still worth a look.
#[test]
fn client_verb_counts_match_their_pins() {
    let root = workspace_root();
    let mut drift = Vec::new();
    for (file, verb, expected, note) in REVIEWED_VERB_COUNTS {
        let path = root.join(file);
        assert!(
            path.is_file(),
            "FAIL CLOSED: pinned file {file} does not exist — a count of 0 here would be vacuous"
        );
        let text = fs::read_to_string(&path).expect("readable");
        let actual = code_only(&text).matches(verb).count();
        if actual != *expected {
            drift.push(format!(
                "  {file}: `{verb}` reviewed {expected}, found {actual}  ({note})"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "Verb counts moved in {} place(s):\n{}\n\n\
         A verb is where a raw client error is born. If you added a network call, route its error \
         through the crate's sealer and update the count here in the same commit. This test is a \
         CHANGE DETECTOR, not a quality bar — it does not know whether your new call is safe, only \
         that nobody has said so.",
        drift.len(),
        drift.join("\n")
    );
}
