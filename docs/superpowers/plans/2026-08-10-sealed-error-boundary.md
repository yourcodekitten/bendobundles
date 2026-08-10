# Sealed Error Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it structurally impossible for a foreign HTTP client's error — which renders the request URL — to be stored in a workspace error type or minted outside a sealing function, and make the guard fail loudly when its own population definition goes stale.

**Architecture:** Seal at the error **type**, not the log **site**. `humble-client`'s 21 raw-error verbs collapse onto two private helpers so the crate contains exactly one `.send()` and one `.bytes()`; `HumbleError::Network` stops storing `wreq::Error`. A committed census test in `domain` then enforces three invariants — a URL-bearing type may only be *named* at a reviewed site, a producer crate may only *mint* one at a reviewed verb count, and every direct dependency carries a reviewed verdict — each with a control proving the check can still fail.

**Tech Stack:** Rust 2024 workspace, `thiserror`, `wreq` 5.3.0, `reqwest` 0.12.28, `tracing`. Census test uses **std only** — no new dependency, production or dev.

## Global Constraints

- **No new dependencies**, `[dependencies]` or `[dev-dependencies]`. The census parses Cargo.toml with std.
- **All commits GPG-signed** (`-S`), authored `code kitten <yourcodekitten@gmail.com>`.
- **CI gates that must pass:** `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
- **THIS BOX CANNOT LINK `cargo test` FOR THE HEAVY CRATES** — a workspace test link needs ≥1638M (measured 2026-08-07); free memory at plan time was **882M** with six sibling tenants on the metal. Local gate for those is `cargo check --workspace --all-targets -j 1` (413M peak) and `cargo clippy … -j 1` (468M peak); **their test evidence comes from CI only — never report a local pass you did not see.**
- ✅ **BUT THE CENSUS TEST RUNS LOCALLY, AND THAT IS MEASURED, NOT HOPED.** `cargo test -p domain --no-run -j 1` on 2026-08-10T07:3x: **peak RSS 317,548 kB, 26.8 s, exit 0.** That is why the census lives in `domain` (3 direct deps) rather than next to the doctrine in `fulfillment` — **it makes the four sabotage controls in Task 3 Step 3 a 27-second local loop instead of four CI round-trips**, and a control that is expensive to run is a control that gets skipped. If it ever *does* OOM (a sibling tenant's build can take the headroom), say so plainly and take that evidence from CI.
- **`HumbleError::Network`'s `Display` text must not change** (`"network error talking to humble: {0}"`), so no existing assertion on the message needs editing. If a test breaks on message text, that is a signal the change was wider than intended.
- Per #185 (still open), `cargo test --workspace` runs **without `--no-fail-fast`**, so a green CI run must be confirmed by finding the **named test binaries** in the log, not by the check being green.
- **Every census check fails closed.** A check that cannot distinguish "found nothing" from "could not look" is the defect this plan exists to remove.

---

### Task 1: The name census, red on today's tree

**Files:**
- Create: `crates/domain/tests/sealed_error_boundary.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `URL_BEARING: &[&str]`, `REVIEWED_OCCURRENCES: &[(&str, &str, &str)]`, `fn workspace_root() -> PathBuf`, `fn rust_sources() -> Vec<PathBuf>`, `fn occurrences_of(needle: &str) -> Vec<(String, usize, String)>` — Task 3 adds tests to this same file and reuses all five.

- [ ] **Step 1: Write the census with its fail-closed controls**

Create `crates/domain/tests/sealed_error_boundary.rs`:

```rust
//! WORKSPACE INVARIANT — a URL-bearing foreign error type may be NAMED only where it is sealed.
//!
//! Doctrine: `crates/fulfillment/src/operator_message.rs` module header. Origin: #173, where this
//! class was found for the third time, having been hand-fixed twice.
//!
//! WHY THIS LIVES IN `domain`: it is a workspace-wide invariant, and `domain` has three direct
//! dependencies, so this binary links where a `fulfillment` test binary will not.
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
```

- [ ] **Step 2: Run it and confirm it FAILS, naming the real defect**

Run: `cargo test -p domain --test sealed_error_boundary -j 1`

Expected: the two control tests **PASS**; `url_bearing_error_types_are_named_only_where_they_are_sealed` **FAILS** naming exactly one unreviewed site:

```
crates/humble-client/src/lib.rs:264
    Network(#[from] wreq::Error),
```

**This failure is the labelled positive for the whole arc** — the census's first output is the real, open, independently-filed defect (#173). Record the verbatim failure text; it is the evidence that the checker discriminates.

If the link OOMs on this box, record that plainly and carry the check to CI — do not claim a local pass.

- [ ] **Step 3: Commit the red census**

```bash
git add crates/domain/tests/sealed_error_boundary.rs
git commit -S -m "test: census — a URL-bearing client error may be named only where sealed

Red on purpose. Its first output is #173's open defect,
humble-client/src/lib.rs:264 \`Network(#[from] wreq::Error)\`, which is the
labelled positive proving the checker discriminates. Ships with a positive
control (scanner sees a string that IS there) and a negative control, because
a census that cannot fail is a report, not a check."
```

---

### Task 2: Seal `humble-client` — census goes green

**Files:**
- Modify: `crates/humble-client/src/lib.rs:264` (the variant), plus the 21 verb sites (9 `.send()`, 12 `.bytes()`)
- Modify: `crates/domain/tests/sealed_error_boundary.rs` (`REVIEWED_OCCURRENCES` gains the new sealing site)

**Interfaces:**
- Consumes: `REVIEWED_OCCURRENCES` from Task 1.
- Produces: `HumbleError::Network(String)`; private `fn net(e: wreq::Error) -> HumbleError`; private `async fn send(&self, rb: wreq::RequestBuilder) -> Result<wreq::Response, HumbleError>`; private `async fn body(resp: wreq::Response) -> Result<Vec<u8>, HumbleError>`. Task 3 pins the verb counts these produce.

- [ ] **Step 0: Add the correlator BEFORE removing the URL that currently carries it**

**Order is load-bearing — do this step first.** `fulfillment/src/lib.rs:1077` logs `claim_id` and
*not* `gamekey`, so today the gamekey reaches CloudWatch **only** inside the error's URL. Sealing
first would leave a window where that path has the correlator in neither place.

In `crates/fulfillment/src/lib.rs:1077`, add the field the way `:3392` already does:

```rust
            tracing::warn!(claim_id, gamekey = %gamekey, error = ?e, "choice pre-read order failed — parking (no spend)");
```

`gamekey` is in scope — `deps.humble.order(gamekey)` is the call whose error this arm handles.

Run: `cargo check -p fulfillment -j 1` — expected: clean.

```bash
git add crates/fulfillment/src/lib.rs
git commit -S -m "fix(fulfillment): log gamekey explicitly at the choice pre-read failure

:1077 carried claim_id and not gamekey, while :3392 carries gamekey and not
claim_id. Two sites, two paths, neither carrying both — so at :1077 the gamekey
reached CloudWatch only as a side effect of the error's unredacted URL.

#173 said the gamekey was 'already logged beside claim_id' and that sentence was
false; this spec inherited it and nearly spent a correlator while citing its own
leak as proof it was free. A correlator you can only get out of an unredacted URL
was never a correlator. Found by Lilith, verified here."
```

- [ ] **Step 1: Change the variant and add the sealer**

In `crates/humble-client/src/lib.rs`, replace line 264:

```rust
    #[error("network error talking to humble: {0}")]
    Network(String),
```

Note the `Display` template is **unchanged** — `{0}` is now a `String` instead of a `wreq::Error`, so no test asserting on the message needs editing.

Add near the error enum:

```rust
/// The ONLY place in this crate where a `wreq::Error` becomes a `HumbleError`.
///
/// Strips the request URL before stringifying. Order fetches embed the gamekey as
/// `/api/v1/order/{gamekey}?all_tpkds=true`, and `wreq::Error`'s `Display` appends the URL
/// (`error.rs:229-230`) while its `Debug` prints the `url` field (`error.rs:198-199`).
/// `without_url()` (`error.rs:77`) drops the url and keeps the kind — the half with diagnostic
/// value. Same remedy as `crates/steam-client/src/lib.rs`'s `fn net`, and the reason this one is
/// reachable from only two call sites is #173: a sealer that must be remembered at 21 sites is the
/// same dependency on memory as no sealer.
fn net(e: wreq::Error) -> HumbleError {
    HumbleError::Network(e.without_url().to_string())
}
```

- [ ] **Step 2: Collapse the 21 verbs onto two helpers**

Add as private methods on the client type that owns `http: wreq::Client` (`lib.rs:589`):

```rust
    /// The only `.send()` in this crate. See `fn net`.
    async fn send(&self, rb: wreq::RequestBuilder) -> Result<wreq::Response, HumbleError> {
        rb.send().await.map_err(net)
    }

    /// The only `.bytes()` in this crate. See `fn net`.
    async fn body(resp: wreq::Response) -> Result<Vec<u8>, HumbleError> {
        resp.bytes().await.map(|b| b.to_vec()).map_err(net)
    }
```

**⚠️ READ THIS BEFORE TOUCHING A LINE — THE SITES ARE NOT UNIFORM, AND THE THREE THAT MATTER MOST
ARE THE THREE THE COMPILER WILL NEVER MENTION.**

An earlier draft of this step said "rewrite each of the 12 `.bytes()` sites to `Self::body(resp).await?`"
and trusted `#[from]`'s deletion to enumerate the rest. **Both halves were wrong.** Deleting the `From`
impl only breaks sites that *convert* — and **three sites render a raw `wreq::Error` without ever
converting one**, so the compiler stays silent about exactly the lines that leak:

| site | current code | why the compiler misses it |
|---|---|---|
| **`:977`** | `tracing::warn!(error = %e, "csrf preflight GET failed")` | `.send()` with **no `?`** — a `match` on the `Result`. **This is byte-for-byte the shape #171 fixed in `deliver()`**: raw client error, `%` Display, which appends the request URL. |
| **`:1436`** | `Err(e) => (false, Some(e.to_string()))`, logged at `:1460` as `body_read_err` | matches the `Result`; no conversion, no type path on the line |
| **`:1548`** | same, logged at `:1568` | same |

**Measured, so nobody has to guess the severity:** `:977`'s URL is `format!("{}/", self.base)` and
`:1436`/`:1548`'s is `{base}/humbler/redeemkey` — the gamekey and `machine_name` travel in the **POST
form body**, not the URL. **So none of the three leaks a credential today.** They are the same class as
`keyed_json`'s comment: *true today, guaranteed by nothing.*

**The verb collapse is what found them.** The name census cannot see them (no type path), and the
`#[from]` deletion cannot see them (no conversion). Enumerating the verbs to collapse them is the only
one of the three mechanisms in this plan that put a human's eye on those lines — which is the
argument for the verb census, discovered by the plan review rather than asserted in the spec.

**Now the rewrite — all 21 sites route through the two helpers, and the three above are fixed *by*
routing**, because `HumbleError`'s own `Display` is already sealed. No inline `without_url()` needed
anywhere.

**The 9 `.send()` sites.** Eight are `…builder….send().await?` → `self.send(…builder…).await?`
(`653, 727, 1243, 1354, 1503, 1622, 1658, 1681`). The ninth, **`:972`**, is a `match` and becomes:

```rust
        let resp = match self
            .send(
                self.http
                    .get(format!("{}/", self.base))
                    .header("Cookie", self.session_cookie()),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                // `e` is now a HumbleError, whose Display is already URL-stripped by `net`.
                tracing::warn!(error = %e, "csrf preflight GET failed");
                return None;
            }
        };
```

**The 12 `.bytes()` sites**, by shape — every one becomes `Self::body(resp)`, and the differing tails
stay exactly as they are:

- **converts via `?`** (`657, 730, 1256, 1370, 1516`): `resp.bytes().await?` → `Self::body(resp).await?`.
  `:730` keeps its borrow: `Ok(String::from_utf8_lossy(&Self::body(resp).await?).into_owned())`.
- **discards deliberately** (`985, 1665`): `let _ = resp.bytes().await;` → `let _ = Self::body(resp).await;`
  (both are connection-pool drains; the comments above them say so — keep them).
- **defaults on error** (`1627, 1688`): `.unwrap_or_default()` → `Self::body(resp).await.unwrap_or_default()`.
  `Vec<u8>::default()` is `vec![]`, matching the previous `Bytes::default()` behaviour at every use,
  all of which take `&bytes`.
- **matches the Result** (`1291, 1436, 1548`): `resp.bytes().await` → `Self::body(resp).await`.
  `:1291`'s `matches!(…, Ok(b) if is_login_required(&b))` is unchanged in meaning. `:1436`/`:1548`'s
  `Err(e) => (false, Some(e.to_string()))` **now stringifies a `HumbleError`**, i.e.
  `"network error talking to humble: <stripped>"` — the field keeps its meaning and loses the URL.

**After this step the crate contains exactly one `.send()` and one `.bytes()`, both inside the
helpers, so a raw `wreq::Error` cannot be bound anywhere else.** That is the claim Task 3 pins.

- [ ] **Step 2b: Let the compiler confirm the list, and treat any surprise as a finding**

Run: `cargo check -p humble-client --all-targets -j 1`

Expected: `E0277 the trait bound HumbleError: From<wreq::Error> is not satisfied` at **exactly the
sites not yet rewritten**, and clean once all 21 are done. **If the compiler names a site not in the
tables above, stop and record it** — it means a 22nd verb reached a conversion by a path this review
did not find, and the pinned counts in Task 3 are wrong. Do not pattern-match a fix onto it.

- [ ] **Step 3: Add the new sealing site to the allow-list**

In `crates/domain/tests/sealed_error_boundary.rs`, add to `REVIEWED_OCCURRENCES`:

```rust
    (
        "crates/humble-client/src/lib.rs",
        "fn net(e: wreq::Error)",
        "SEALING SITE: strips the url before stringifying (#173)",
    ),
```

- [ ] **Step 4: Run the census and the crate's own suite**

Run: `cargo test -p domain --test sealed_error_boundary -j 1` — expected: **all four tests PASS**,
including `every_reviewed_occurrence_still_matches_a_real_line` (which now proves the new sealing
site is real, and would have caught a typo in the entry).

Run: `cargo test -p humble-client -j 1` — expected: the existing `client_test.rs` suite passes
**unchanged**. If it OOMs here, say so and take it from CI.

Run: `cargo clippy -p humble-client -p domain --all-targets -- -D warnings -j 1` — expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/humble-client/src/lib.rs crates/domain/tests/sealed_error_boundary.rs
git commit -S -m "fix(humble-client): seal the wreq error boundary (closes #173)

Network(#[from] wreq::Error) -> Network(String), built only by fn net(), which
calls without_url(). Deleting #[from] deletes the From impl, so ? stops
auto-converting and the compiler enumerated every site instead of a grep.

The 21 verbs that can mint a raw wreq::Error (9 .send(), 12 .bytes()) now route
through two private helpers, so the crate contains exactly one of each. That is
what makes the invariant hold for code nobody has written yet — steam-client has
had a sealer since #171 and 13 unforced call sites, which is the same dependency
on memory with a nicer spelling.

Display text is unchanged, so no message assertion moved."
```

---

### Task 2b: Drag the guarantor inside the repo — `steam-client`'s `Parse` path

**Files:**
- Modify: `crates/steam-client/src/lib.rs:921-930` (`keyed_json`'s doc comment and its `Parse` arm)

**Interfaces:**
- Consumes: `fn net` (already present at `:936`).
- Produces: nothing.

**Why this is in scope when the rest of `steam-client` is not.** There is **no leak today** — measured
in `reqwest` 0.12.28: `json()`/`bytes()` errors go through `error::decode`, which attaches no url, and
`response.rs`/`body.rs` contain zero `with_url` callers. But `keyed_json`'s doc comment asserts *"The
key never appears in any error string"* — a by-construction claim that is true **only** because of that
upstream internal. **A safety property whose guarantor is a dependency's private implementation has no
owner in this repo**, and a `reqwest` patch release could falsify the comment with no diff here and no
test to fail. Two lines drags the guarantor inside.

- [ ] **Step 1: Seal the `Parse` arm and make the comment say why it is true**

Replace `keyed_json`'s doc comment and `200` arm in `crates/steam-client/src/lib.rs`:

```rust
/// Shared keyed-endpoint status mapping: 429 → RateLimited, 401/403 → KeyRejected,
/// other non-2xx → Api(status), body → serde or Parse.
///
/// The key never appears in any error string, and THIS FUNCTION is why — not `reqwest`. Both error
/// paths out of here go through `without_url()`. Measured on reqwest 0.12.28: a `json()`/`bytes()`
/// error is built by `error::decode`, which attaches no url, so stripping is a no-op *today* — but
/// that is an upstream implementation detail, not a promise. A patch release that attached the url
/// would have falsified the sentence above with no diff in this repo and no test to fail. Keyed
/// endpoints embed `?key=...`, so the property is worth owning here rather than borrowing.
async fn keyed_json<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, SteamError> {
    match resp.status().as_u16() {
        200 => resp
            .json::<T>()
            .await
            .map_err(|e| SteamError::Parse(e.without_url().to_string())),
        429 => Err(SteamError::RateLimited),
        401 | 403 => Err(SteamError::KeyRejected),
        s => Err(SteamError::Api(s)),
    }
}
```

- [ ] **Step 2: Verify**

Run: `cargo check -p steam-client --all-targets -j 1` — expected: clean.
Run: `cargo clippy -p steam-client --all-targets -- -D warnings -j 1` — expected: clean.
Run: `cargo test -p steam-client -j 1` — expected: `client_test.rs` passes unchanged (`Parse`'s
`Display` template is untouched). If it OOMs, take it from CI.

- [ ] **Step 3: Commit**

```bash
git add crates/steam-client/src/lib.rs
git commit -S -m "fix(steam-client): seal keyed_json's Parse path; own the property instead of borrowing it

Network got without_url() in #171. Parse, nine lines above it on the same
response in the same file, did not — and the function's own doc comment asserts
'The key never appears in any error string'.

That claim is TRUE and it is true because of a reqwest internal: json()/bytes()
errors are built by error::decode, which attaches no url (verified in 0.12.28;
zero with_url callers in response.rs/body.rs). No leak today. But a
by-construction claim resting on an upstream implementation detail is a claim
with no owner — a patch release could falsify the comment with no diff here and
no test to fail. Two lines brings the guarantor in-repo.

Found by Lilith from the render-site end; the payload question was mine to
measure and it came back benign."
```

---

### Task 3: The staleness half — verb counts and dependency verdicts

**Files:**
- Modify: `crates/domain/tests/sealed_error_boundary.rs`

**Interfaces:**
- Consumes: `workspace_root()`, `rust_sources()`, `occurrences_of()` from Task 1; the helper names from Task 2.
- Produces: nothing consumed downstream.

- [ ] **Step 1: Add the verb-count census**

Append to `crates/domain/tests/sealed_error_boundary.rs`:

```rust
/// A raw client error can only come INTO EXISTENCE at one of these verbs, inside a crate that
/// directly depends on the client. Counting them needs no type inference — which a name census
/// does need, and does not have.
const MINTING_VERBS: &[&str] = &[".send()", ".bytes()", ".text()", ".json"];

/// Reviewed verb counts per producer crate: (crate, verb, count, note).
/// A count that MOVES fails this test, which is exactly the event that needs a human: someone
/// added a network call, and its error needs sealing.
const REVIEWED_VERB_COUNTS: &[(&str, &str, usize, &str)] = &[
    ("humble-client", ".send()", 1, "sealed: the only .send() is fn send(), which map_err(net)s"),
    ("humble-client", ".bytes()", 1, "sealed: the only .bytes() is fn body(), which map_err(net)s"),
    ("humble-client", ".text()", 0, "unused in this crate"),
    ("humble-client", ".json", 0, "bodies are parsed with serde_json::from_slice, not .json()"),
    ("steam-client", ".send()", 10, "UNFORCED — fn net exists, nothing requires it; see the steam-client follow-up issue"),
    ("steam-client", ".bytes()", 1, "UNFORCED — same follow-up"),
    ("steam-client", ".text()", 1, "UNFORCED — same follow-up"),
    ("steam-client", ".json", 1, "the keyed_json Parse path, sealed in Task 2b; the other verbs are still unforced"),
    ("fulfillment", ".send()", 2, "one is deliver()'s sealed boundary; the other is a test helper"),
    ("fulfillment", ".bytes()", 0, "unused"),
    ("fulfillment", ".text()", 0, "unused"),
    ("fulfillment", ".json", 1, "reviewed: not an error-rendering path"),
];

#[test]
fn producer_crates_mint_raw_client_errors_only_at_reviewed_verbs() {
    let root = workspace_root();
    let mut drift = Vec::new();
    for (krate, verb, expected, note) in REVIEWED_VERB_COUNTS {
        let src = root.join("crates").join(krate).join("src");
        assert!(
            src.is_dir(),
            "FAIL CLOSED: {} is not a directory — a count of 0 here would be vacuous",
            src.display()
        );
        let actual: usize = rust_sources()
            .iter()
            .filter(|p| p.starts_with(&src))
            .map(|p| {
                fs::read_to_string(p)
                    .unwrap_or_else(|e| panic!("FAIL CLOSED: cannot read {}: {e}", p.display()))
                    .matches(verb)
                    .count()
            })
            .sum();
        if actual != *expected {
            drift.push(format!(
                "  {krate}: `{verb}` reviewed {expected}, found {actual}  ({note})"
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "Verb counts moved in {} place(s):\n{}\n\n\
         A verb is where a raw client error is born. If you added a network call, route its error \
         through the crate's sealer (`fn net`) and update the reviewed count here in the same \
         commit. If you removed one, update the count. This test is a change detector, not a \
         quality bar — it does not know whether your new call is safe, only that nobody has said so.",
        drift.len(),
        drift.join("\n")
    );
}
```

- [ ] **Step 2: Add the dependency-verdict census, with the two-syntax parser**

Append:

```rust
/// Verdicts for every DIRECT dependency of every workspace crate.
///
/// Population argument: a workspace error enum can only STORE a type it can NAME, and it can only
/// name a type from a direct dependency. Measured 2026-08-10: the workspace has zero `anyhow`,
/// zero `eyre` and zero `Box<dyn Error>` — no erasure hatch through which a transitive crate's
/// error could arrive unnamed. So "review every direct dependency" is exhaustive for the storage
/// half, not a heuristic hoping to spot clients.
///
/// `UrlBearing` = its error type can render a request URL; must never be stored, only converted.
/// `ReviewedSafe` = error type reviewed; cannot carry a URL or credential.
/// `NoErrorTypeHeld` = we hold no error type of its.
///
/// THE THIRD FIELD IS THE RE-EXPORT SIGNATURE, AND IT IS A FIELD RATHER THAN A CAVEAT ON PURPOSE.
/// This checker is syntactic, so it cannot see a foreign error type that arrives under another name
/// because a direct dependency re-exports it (`dep::TheirError`). That gap was going to live in this
/// module's doc comment until Lilith pointed out the obvious: **a caveat in a test's doc comment is
/// the worst available location, because the GREEN is what gets read and the comment is furniture.**
/// So it is a value the same test enforces: a row may not be `ReviewedSafe` with an empty signature.
/// A blind spot that ships becomes a row somebody signed.
const DEP_VERDICTS: &[(&str, &str, &str)] = &[
    ("argon2", "ReviewedSafe", "re-exports checked: none"),
    ("async-trait", "NoErrorTypeHeld", "re-exports checked: none (proc-macro)"),
    ("aws-config", "ReviewedSafe", "re-exports checked: aws-smithy types; captured as a String at dynamo/src/lib.rs:310,338 via format!(\"{sdk_err:?}\") — 16 sites, see #151"),
    ("aws-sdk-dynamodb", "ReviewedSafe", "re-exports checked: aws-smithy SdkError; same 16 capture sites, see #151"),
    ("aws-sdk-lambda", "ReviewedSafe", "re-exports checked: aws-smithy SdkError"),
    ("aws-sdk-ssm", "ReviewedSafe", "re-exports checked: aws-smithy SdkError"),
    ("axum", "ReviewedSafe", "re-exports checked: http/hyper error types, not stored by us"),
    ("data-encoding", "ReviewedSafe", "re-exports checked: none"),
    ("hmac", "NoErrorTypeHeld", "re-exports checked: none"),
    ("lambda_http", "ReviewedSafe", "re-exports checked: http types; adapter errors not stored (see #186)"),
    ("lambda_runtime", "ReviewedSafe", "re-exports checked: same as lambda_http"),
    ("reqwest", "UrlBearing", "sealed at steam-client fn net + keyed_json, fulfillment deliver()"),
    ("serde", "ReviewedSafe", "re-exports checked: none"),
    ("serde_json", "ReviewedSafe", "re-exports checked: none; stored as Parse(serde_json::Error)"),
    ("sha1", "NoErrorTypeHeld", "re-exports checked: none"),
    ("sha2", "NoErrorTypeHeld", "re-exports checked: none"),
    ("thiserror", "NoErrorTypeHeld", "re-exports checked: none (derive only)"),
    ("time", "ReviewedSafe", "re-exports checked: none"),
    ("tokio", "ReviewedSafe", "re-exports checked: io::Error, no URL surface"),
    ("tracing", "NoErrorTypeHeld", "re-exports checked: none"),
    ("tracing-subscriber", "ReviewedSafe", "re-exports checked: none stored"),
    ("urlencoding", "ReviewedSafe", "re-exports checked: none"),
    ("uuid", "ReviewedSafe", "re-exports checked: none"),
    ("wreq", "UrlBearing", "sealed at humble-client fn net"),
    ("wreq-util", "ReviewedSafe", "re-exports checked: wreq types — emitter/profile config only, no error stored"),
];

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
         This checker is syntactic: it cannot see a foreign error type re-exported under another \
         name. The verdict is the compensating control, so it must state that the re-exports were \
         looked at. Write `re-exports checked: <what you found>` (or, for a UrlBearing crate, \
         `sealed at <where>`)."
    );
}

/// Workspace members are excluded from DEP_VERDICTS: their error types are the thing being audited,
/// not a foreign surface to review.
const WORKSPACE_MEMBERS: &[&str] = &[
    "domain",
    "dynamo",
    "fulfillment",
    "humble-client",
    "public-api",
    "admin-api",
    "steam-client",
];

/// Parse the `[dependencies]` section of a Cargo.toml.
///
/// TWO SYNTAXES, AND MISSING ONE IS A SILENT UNDERCOUNT. This workspace writes both
/// `serde.workspace = true` and `tokio = { version = "1", features = [...] }`. A pattern anchored
/// on `^name =` sees only the second and reports `domain` as having NO dependencies — which is how
/// this parser was first written, and it under-reported every crate at once while looking correct.
/// Dev-dependencies are excluded: the property is about production error types.
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
        // Name ends at the first of ' ', '.', '='  — covers `a = ...`, `a.workspace = true`, `a="1"`.
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

/// CONTROL for the parser above: it must see a dependency written in EACH syntax.
#[test]
fn the_manifest_parser_sees_both_dependency_syntaxes() {
    let root = workspace_root();
    let domain = direct_deps(&root.join("crates/domain/Cargo.toml"));
    assert!(
        domain.contains(&"thiserror".to_string()),
        "FAIL CLOSED: parser missed `thiserror.workspace = true` in crates/domain/Cargo.toml — it \
         found {domain:?}. Every 'no unreviewed dependency' verdict below would be vacuous."
    );
    let ful = direct_deps(&root.join("crates/fulfillment/Cargo.toml"));
    assert!(
        ful.contains(&"tokio".to_string()),
        "FAIL CLOSED: parser missed a table-syntax dependency in crates/fulfillment/Cargo.toml — \
         it found {ful:?}."
    );
}

#[test]
fn every_direct_dependency_has_a_reviewed_verdict() {
    let root = workspace_root();
    let mut found: Vec<String> = Vec::new();
    for m in WORKSPACE_MEMBERS {
        let manifest = root.join("crates").join(m).join("Cargo.toml");
        assert!(
            manifest.is_file(),
            "FAIL CLOSED: {} missing — WORKSPACE_MEMBERS is stale and the population is incomplete",
            manifest.display()
        );
        found.extend(direct_deps(&manifest));
    }
    found.retain(|d| !WORKSPACE_MEMBERS.contains(&d.as_str()));
    found.sort();
    found.dedup();
    assert!(
        found.len() >= 20,
        "FAIL CLOSED: only {} foreign direct dependencies parsed — expected ~25. The parser is \
         under-reporting and this test would pass vacuously. Found: {found:?}",
        found.len()
    );

    let reviewed: Vec<&str> = DEP_VERDICTS.iter().map(|(n, _, _)| *n).collect();
    let unreviewed: Vec<&String> = found.iter().filter(|d| !reviewed.contains(&d.as_str())).collect();
    let vanished: Vec<&&str> = reviewed
        .iter()
        .filter(|r| !found.iter().any(|f| f == *r))
        .collect();

    assert!(
        unreviewed.is_empty(),
        "New direct dependency(ies) with no verdict: {unreviewed:?}\n\n\
         Decide, for each: does its error type render a request URL or any credential? If yes, mark \
         it UrlBearing and seal it at its boundary the way crates/humble-client's `fn net` does. If \
         no, mark it ReviewedSafe — and note that verdict also claims its RE-EXPORTS are safe, which \
         is this census's one stated blind spot. Then add it to DEP_VERDICTS."
    );
    assert!(
        vanished.is_empty(),
        "DEP_VERDICTS reviews dependency(ies) the workspace no longer has: {vanished:?}\n\n\
         Remove them. A verdict table that rots in this direction quietly grows the set of names \
         that would pass review without anyone looking."
    );
}
```

- [ ] **Step 3: Run and confirm green, then prove each new check can fail**

Run: `cargo test -p domain --test sealed_error_boundary -j 1`
Expected: **all eight tests PASS** — `the_scanner_can_see_a_string_that_is_definitely_there`,
`the_scanner_reports_nothing_for_a_string_that_is_definitely_absent`,
`url_bearing_error_types_are_named_only_where_they_are_sealed`,
`every_reviewed_occurrence_still_matches_a_real_line`,
`producer_crates_mint_raw_client_errors_only_at_reviewed_verbs`,
`the_manifest_parser_sees_both_dependency_syntaxes`,
`every_direct_dependency_has_a_reviewed_verdict`,
`every_verdict_carries_a_re_export_signature`.

Then run four deliberate sabotage checks, reverting each immediately. **A check never observed
failing is a report, not a check** — and every one of these must be *seen*, not assumed:

1. Add `let _ = "wreq::Error";` to `crates/domain/src/lib.rs` → `url_bearing_error_types_are_named_only_where_they_are_sealed` must FAIL naming that line. Revert.
2. Change a `REVIEWED_VERB_COUNTS` entry (e.g. `humble-client` `.send()` → `2`) → `producer_crates_mint_raw_client_errors_only_at_reviewed_verbs` must FAIL. Revert.
3. Delete `("uuid", "ReviewedSafe", …)` from `DEP_VERDICTS` → `every_direct_dependency_has_a_reviewed_verdict` must FAIL naming `uuid`. Revert.
4. Blank the third field of any `DEP_VERDICTS` row → `every_verdict_carries_a_re_export_signature` must FAIL naming that crate. Revert.

Record the four failure messages verbatim. **Confirm the tree is clean afterwards** (`git status`)
— a sabotage control that is not reverted ships the sabotage.

**And confirm each sabotage failed the test it was aimed at, not merely that something failed.** Four
checks that all report through one binary can mask each other: if #4's edit also happened to trip #3,
"the suite went red" would be evidence for the wrong check.

- [ ] **Step 4: Commit**

```bash
git add crates/domain/tests/sealed_error_boundary.rs
git commit -S -m "test: the staleness half — verb counts and dependency verdicts

A name census cannot see a value bound by inference, so count the VERBS that
mint one (.send/.bytes/.text/.json) per producer crate. A count that moves is
exactly the event needing a human: you added a network call, seal its error.

Plus a verdict for all 25 foreign direct dependencies. Population is exhaustive
for the storage half because a workspace error enum can only store a type it can
name, and the tree has zero anyhow / eyre / Box<dyn Error>.

The manifest parser ships a control that proves it sees BOTH dependency syntaxes.
Written the obvious way it matched only \`name = ...\`, reported domain as having
no dependencies at all, and under-counted every crate while looking correct."
```

---

### Task 4: Put the mechanism where the doctrine already lives, and file the debt

**Files:**
- Modify: `crates/fulfillment/src/operator_message.rs` (module header, after the existing trust-boundary paragraphs)

- [ ] **Step 1: Add the paragraph**

Append to the module header in `crates/fulfillment/src/operator_message.rs`, after the paragraph
ending *"A trust boundary has two sides and both of them need auditing."*:

```rust
//! **BOTH SIDES NOW HAVE A MECHANISM, AND FOR MOST OF THIS FILE'S LIFE ONLY ONE DID.** Everything
//! above is the doctrine — *a filter protects the code you audited; a type protects the code you
//! haven't* — and it was implemented on the Discord side only. The CloudWatch side had 49
//! hand-written `error = ?e` / `%e` sites and two hand-applied `without_url()` calls covering three
//! places that needed one. The third (`humble-client`'s `Network(#[from] wreq::Error)`, #173) sat
//! open through two arcs that quoted this header while it did.
//!
//! The CloudWatch side is now sealed at the ERROR TYPE instead of the log site, because log sites
//! are unbounded and error types are not: a raw client error is converted by a single `fn net` per
//! client crate, and the verbs that can mint one are collapsed onto one helper each. The invariant
//! is enforced by `crates/domain/tests/sealed_error_boundary.rs`, which also fails when its own
//! population definition goes stale — a new HTTP call, a new direct dependency, or an allow-list
//! entry that no longer matches any line. **Read that file's header before adding a network call or
//! a dependency; it states its own blind spot, which is more than this one managed for two arcs.**
```

- [ ] **Step 1b: 🔴 THE PARAGRAPH YOU JUST WROTE TRIPS THE CENSUS. Allow-list it in the same commit.**

The text above contains the literal string **`wreq::Error`** (in *"`humble-client`'s
`Network(#[from] wreq::Error)`, #173"*). `crates/fulfillment/src/operator_message.rs` is scanned by
`url_bearing_error_types_are_named_only_where_they_are_sealed`, which does **not** strip comments — by
design, since a comment-stripper's failure mode is a false negative. **So the doc paragraph is a new
unreviewed occurrence and the census goes red on it.**

This is the census working exactly as intended and it is still a trap for whoever executes this task,
because the failure will look like the *documentation* broke the build. Add to `REVIEWED_OCCURRENCES`
in `crates/domain/tests/sealed_error_boundary.rs`, **in the same commit as the paragraph**:

```rust
    (
        "crates/fulfillment/src/operator_message.rs",
        "`Network(#[from] wreq::Error)`, #173",
        "PROSE: the doctrine paragraph naming the defect this mechanism closed",
    ),
```

**Do not fix this by rewording the paragraph to avoid the type name.** The whole point of counting
prose is that discussion of the class is cheap to allow-list and expensive to lose; an author who
learns to dodge the census by paraphrasing has been taught the wrong lesson by it.

- [ ] **Step 2: Verify nothing else moved, and that the census is green *because* it was updated**

Run: `cargo fmt --check` and `cargo clippy -p fulfillment --all-targets -- -D warnings -j 1`
Expected: clean. (A doc comment cannot change behaviour, but `fmt` has opinions about comment width.)

Run: `cargo test -p domain --test sealed_error_boundary -j 1` — expected: **all eight PASS.**

**Then confirm the allow-list entry is load-bearing rather than decorative:** comment it out, re-run,
and check the census FAILS naming `operator_message.rs`. Restore it. An entry that changes nothing
when removed was matching something else.

- [ ] **Step 3: Commit**

```bash
git add crates/fulfillment/src/operator_message.rs crates/domain/tests/sealed_error_boundary.rs
git commit -S -m "docs: the doctrine's own file now names the mechanism on both sides

This header carried 'a filter protects the code you audited; a type protects the
code you haven't' while guarding one of its two sinks with a hand-applied filter.
Points at the census, and tells the next author to read a file that states its
own blind spot."
```

- [ ] **Step 4: File the `steam-client` debt as its own issue**

```bash
gh issue create -R yourcodekitten/bendobundles \
  --title "steam-client: fn net() is a sealer nothing forces — 13 unforced verb sites" \
  --body "Measured 2026-08-10 while sealing humble-client (#173).

\`crates/steam-client/src/lib.rs:936\` has had \`fn net(e: reqwest::Error)\` since #171, which
strips the URL correctly and says why in a comment. It is not the problem.

The problem is that **nothing requires any call site to use it.** The crate has 13 sites that mint a
raw \`reqwest::Error\` — 10 \`.send()\`, 1 \`.json\`, 1 \`.text()\`, 1 \`.bytes()\` — and the
property holds only while every author remembers. That is the same dependency on memory as having no
sealer, one step further along, and it is why #173 argued for a mechanism instead of a third hand-fix.

**Nothing stored is unsealed** — \`SteamError::Network\` is \`String\` and \`Parse\` was sealed
alongside this measurement, so this is not a live leak and not urgent. What is unforced is the
conversion: the sealing happens to be applied everywhere it matters *today*, by hand, at 13 sites.

**The fix is the one just applied to humble-client:** collapse the verbs onto private helpers so the
crate contains exactly one \`.send()\` / \`.bytes()\` / \`.text()\` / \`.json\`, each sealing inside.
\`crates/domain/tests/sealed_error_boundary.rs\` pins these counts at today's 10/1/1/1 with this
issue named, so the guard records the debt rather than implying it away — closing this issue means
changing those counts to 1/1/1/1.

Deliberately not bundled into the sealing PR: a second crate of mechanical churn would make that
diff argue for itself less clearly." \
  --label enhancement
```

Then add the issue number to the four `steam-client` rows in `REVIEWED_VERB_COUNTS`, and commit:

```bash
git add crates/domain/tests/sealed_error_boundary.rs
git commit -S -m "test: name the steam-client follow-up issue in the pinned verb counts"
```

---

## Self-review

**Spec coverage:**

| Spec item | Task |
|---|---|
| Mechanism 1 — seal `humble-client`, `Network(String)` + `fn net` | Task 2 steps 1–2 |
| Collapse 21 verbs onto two helpers | Task 2 step 2 |
| Mechanism 2(a) — name allow-list, comments included | Task 1 |
| Mechanism 2(b) — dependency verdicts + staleness both directions | Task 3 step 2 |
| Verb-count census (the inference hole) | Task 3 step 1 |
| Census must be shown to fail / labelled positive | Task 1 step 2 (red on the real defect) + Task 3 step 3 (three sabotage controls) |
| Mechanism 3 — doctrine paragraph | Task 4 steps 1–3 |
| Non-goal: `steam-client`'s 13 verbs scoped out, debt recorded | Task 4 step 4 |
| Non-goal: 49 log sites untouched | no task, by design |
| Review §1 — `keyed_json`'s `Parse` path; guarantor dragged in-repo | Task 2b |
| Review §1 — the `gamekey` correlator that `without_url()` was about to spend | Task 2 step 0 |
| Review §2 — re-export gap as a signed verdict field, not a doc comment | Task 3 step 2 (`every_verdict_carries_a_re_export_signature`) |
| Review §3 — dynamo's 16 AWS Debug captures named so the verdict is asked | Task 3 step 2 (`aws-*` rows cite `dynamo:310,338` and #151) |

**Placeholder scan:** no TBD/TODO; every code step carries the code; every run step carries the exact
command and the expected result.

**Type consistency:** `net` / `send` / `body` are named identically in Task 2 and referenced by those
names in Task 3's `REVIEWED_VERB_COUNTS` notes and Task 4's paragraph. `occurrences_of`,
`rust_sources`, `workspace_root`, `direct_deps` are defined once in Task 1/3 and reused unchanged.
`REVIEWED_OCCURRENCES` is a 3-tuple in Task 1 and gains a 3-tuple entry in Task 2 step 4.

**Known open input:** the two questions in the shared channel (`without_url()`'s diagnostic trade at
the Humble boundary; whether documenting the re-export blind spot is sufficient). Neither changes the
task structure — the first changes one line inside `fn net`, the second changes a doc comment and
possibly adds a verdict column. **Do not block execution on them; integrate the answers when they
arrive.**

---

## Plan review — `implementation-plan-review`, 2026-08-10, cold-subagent walkthrough

**Verdict: ready to execute, after the fixes below — which are already folded in above.** Three
blockers, all found by walking the plan as a context-free subagent and then *checking its claims
against the tree* rather than reading its prose.

### BLOCKER 1 — Task 2 Step 2 told a subagent to rewrite 12 non-uniform sites identically

The step read *"rewrite each of the 12 `.bytes()` sites to `Self::body(resp).await?`"*. Measured: **5**
convert via `?`, **4** deliberately discard (`let _ =` × 2, `.unwrap_or_default()` × 2 — two of them
documented connection-pool drains), and **3** match on the `Result`. A subagent following that
instruction literally would have put `?` on a drain and changed control flow on the step-up paths.
**Fixed:** the step now enumerates all 21 sites by shape with the exact post-change form for each.

### BLOCKER 2 — the mechanism the plan's three checks were all blind to

The `#[from]` deletion only breaks sites that *convert*. **Three sites render a raw `wreq::Error`
without converting one**, so the compiler never names them and the name census cannot see them (no type
path on the line):

- **`crates/humble-client/src/lib.rs:977`** — `tracing::warn!(error = %e, "csrf preflight GET failed")`.
  **Byte-for-byte the shape #171 fixed in `deliver()`**: raw client error, `%` Display, URL appended.
- **`:1436`** and **`:1548`** — `Err(e) => (false, Some(e.to_string()))`, logged as `body_read_err` at
  `:1460` / `:1568`.

**Measured severity: no credential leaks today.** `:977`'s URL is `format!("{}/", self.base)`;
`:1436`/`:1548` post to `{base}/humbler/redeemkey` with the gamekey and `machine_name` in the **form
body**. Same class as `keyed_json`'s comment — true today, guaranteed by nothing.

**The plan would have shipped a census, a doctrine paragraph, and "closes #173" while leaving the most
direct instance of the class untouched, 700 lines above the variant it was fixing.** All three are
fixed *by* routing through the helpers, since `HumbleError`'s `Display` is sealed. **What found them was
enumerating the verbs in order to collapse them** — the verb census is the only one of the three
mechanisms that put an eye on those lines, which is an argument the spec asserted and the review
earned.

### BLOCKER 3 — Task 4's own paragraph turns the census red, and the failure blames the docs

The doctrine paragraph contains the literal `wreq::Error`, `operator_message.rs` is scanned, and the
checker deliberately does not strip comments. **Fixed:** Step 1b adds the allow-list entry in the same
commit, with an instruction *not* to dodge the census by rewording — plus a check that the new entry is
load-bearing (comment it out, confirm red, restore).

### MINOR — the pinned counts and the census count things differently

`REVIEWED_VERB_COUNTS` was measured with `grep -c` (**lines**) and the test uses `str::matches`
(**occurrences**). Cross-checked both ways on 2026-08-10 and every figure agrees (9/9, 12/12, 10/10,
1/1, 2/2, 1/1), so the pins are correct — but they would diverge the day someone writes two `.send()`
on one line. The census's occurrence semantics are the right ones; noted here so a future reader does
not "fix" the numbers back to line counts.

### Checked and clean (claims the plan makes that I verified rather than trusted)

- No `[dependencies.x]` or `[target.…]` sections and no multi-line/indented continuation lines in any
  of the 7 manifests — so `direct_deps`'s narrow parser cannot invent a dependency named `features`.
- No existing `fn send` / `fn body` in `humble-client` to collide with the helpers.
- The `enhancement` label exists in `yourcodekitten/bendobundles`.
- All four `REVIEWED_OCCURRENCES` snippets from Task 1 match real lines today
  (`steam-client:936,938`, `operator_message.rs:15`, `fulfillment:4525`).
- `Vec<u8>` is a safe return for `body()`: every consuming site takes `&bytes`, and
  `Vec::default()` matches the previous `Bytes::default()` behaviour at both `.unwrap_or_default()`
  sites.

### Open question that survives the review

**q1, with OMBB:** with `gamekey = %gamekey` now explicit at `:1077`, is there anything left in a
Humble request URL worth having at 3am? If no, `without_url()` is free everywhere and the last
judgement call in this plan closes. **Does not block execution** — it changes no code in any task,
only whether a future author is told the trade was priced.
