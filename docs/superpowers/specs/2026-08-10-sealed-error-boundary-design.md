# Sealing the error boundary — the CloudWatch sibling of `OperatorMessage`

**Date:** 2026-08-10 · **Author:** code kitten · **Status:** spec, pre-plan
**Closes:** #173 · **Gives a mechanism to the class in:** #151, #172

---

## The finding this starts from

`crates/fulfillment/src/operator_message.rs` opens with a doctrine it earned the hard way:

> SECURITY: `OperatorMessage` has a private inner, NO public string constructor and NO
> `From<String>`. That is deliberate and load-bearing: **a type that *offers* safety is not a type
> that *enforces* it, and only enforcement has jurisdiction over call sites nobody has written yet.**
>
> … **THE PRIVILEGED SIDE IS NOT A SAFE HARBOUR, AND THIS PARAGRAPH USED TO SAY IT WAS.** … An error
> carrying a bearer token has **NO safe side**, and no access-control tier makes it one.

There are **two** sinks through which an error payload leaves this process:

| Sink | Audience | Defence today |
|---|---|---|
| Discord operator channel | unprivileged | **a type** — `OperatorMessage`, private inner, no `From<String>`, 55 call sites, `ErrorSummary::of` never reads the error's text |
| CloudWatch (`tracing`) | access-controlled | **a comment** — 49 hand-written `error = ?e` / `%e` sites (38 `?`, 11 `%`) across 7 crates, and exactly **two** hand-applied `.without_url()` calls in the entire workspace |

**The file that says a filter protects only the code you audited protects one of its two sinks with
a type and the other with a filter it applied by hand, twice, out of three places that needed it.**
That asymmetry is the whole subject of this spec. It is not a criticism of that module — the module
is where the doctrine came from. It is that the doctrine was only ever implemented on one side.

### The class, and its score so far

Three discoveries, two hand-defences, one still open (all verified in-tree on 2026-08-10):

| Site | State |
|---|---|
| `crates/steam-client/src/lib.rs:939` — `fn net(e: reqwest::Error)` → `Network(String)` via `e.without_url().to_string()` | ⚠️ **a sealer exists; nothing forces its use** — see below |
| `crates/fulfillment/src/lib.rs:4543` — `error = %e.without_url()` in `deliver()` | ✅ genuinely sealed — `deliver()` is the crate's *only* reqwest error boundary (measured: 1 `Err` arm) |
| `crates/humble-client/src/lib.rs:264` — `Network(#[from] wreq::Error)` | ❌ **open** — takes the error whole |

**The middle row started this spec's life as a ✅ and the measurement demoted it.** `steam-client` has
`fn net`, but it has **13 call sites** that mint a raw `reqwest::Error` (10 `.send()`, 1 `.json`,
1 `.text()`, 1 `.bytes()`), and *nothing forces any of them through `net`*. A sealer that must be
remembered at 13 sites is the same dependency-on-memory as no sealer, one step further along. So the
class is at **three** crates once you stop asking "is there a fix" and start asking "what forces it."

`wreq` 5.3.0 is a `reqwest` lineage fork: `Display` appends the request URL, `Debug` prints the `url`
field, `without_url()` exists. `HumbleError::Network`'s own `Display` is
`"network error talking to humble: {0}"`, so it propagates the inner rendering, and `#[derive(Debug)]`
propagates it through `?e` too.

**What leaks today is a gamekey** — an order identifier, from
`/api/v1/order/{gamekey}?all_tpkds=true` — **not a bearer credential.** The gift key is built from a
response body and never requested; the session cookie travels as a header. #173 calibrated this
honestly as a MAJOR, not a blocker, and this spec keeps that calibration. **The reason to fix it is
that there is no guard**: the day a Humble request carries a token in its URL, this becomes a
credential leak with no type, no test and no lint in the way — and it will look exactly as green as
it looks now.

### Why not a third hand-fix

Because the property would then rest on whoever writes the fourth client remembering. #173 says this
in as many words, and it is the argument #171 spent its whole arc making. **A search tells you what
you found; a mechanism tells you what could not have been missed.**

---

## The property to guarantee

> **No foreign error value whose rendering can contain a request URL is ever stored in a workspace
> error type or rendered at a log site — and the guard fails loudly when the set of foreign
> dependencies it was reviewed against changes.**

Two clauses, because either alone is a hole: sealing the types leaves direct `error = ?raw_client_err`
logging open, and auditing the sites leaves the next author's `#[from]` open.

---

## Approaches considered

### A. Seal at the error TYPE + a self-invalidating census — **recommended**

Invert the axis. Redaction becomes a property of the **error type** (finite, enumerable, 5 enums)
instead of the **log site** (unbounded, and includes sites nobody has written yet).

1. `HumbleError::Network(#[from] wreq::Error)` → `Network(String)`, constructed by a
   `fn net(e: wreq::Error) -> HumbleError` that strips the URL — byte-for-byte the shape
   `steam-client` already uses. **Deleting `#[from]` deletes the `From` impl, so `?` stops
   auto-converting and the compiler now demands an explicit decision at every call site.** The
   enforcement for this crate is the absence of a conversion, not a lint.
2. **Collapse the 21 verbs to one.** Rather than sprinkle `.map_err(net)` across 21 sites — which
   recreates steam-client's remember-it-every-time situation with a nicer spelling — route them
   through two private helpers, so the crate contains exactly **one** `.send()` and **one**
   `.bytes()`:

   ```rust
   /// The only place in this crate where a `wreq` error can come into existence.
   async fn send(&self, rb: wreq::RequestBuilder) -> Result<wreq::Response, HumbleError> {
       rb.send().await.map_err(net)
   }
   async fn body(resp: wreq::Response) -> Result<Vec<u8>, HumbleError> {
       resp.bytes().await.map(|b| b.to_vec()).map_err(net)
   }
   ```

   This is why the verb-count census is cheap to hold: the reviewed count for `humble-client` becomes
   `1` and `1`, and a future author who calls `.send()` directly trips it.
3. A committed census test that fails when its own population definition goes stale (below).

**Cost:** one variant, 21 call sites collapsed onto two helpers, one test module. **Buys:** every
present and future log site is safe without touching any of them, because there is no longer an
unsealed value in the crate to log.

### B. Type the log sink — an `OperatorMessage` analogue for `tracing`

A `LogSafe` wrapper that every `error =` field must go through, plus conversion of all 49 sites.

**Rejected.** It is the bigger diff (49 mechanical edits) for a *weaker* guarantee: the wrapper is
only reached if the author uses it, so it still needs a textual census to enforce, and it leaves the
raw error in the type where the next author can log it another way. It also fights the grain — a
shared wrapper needs a shared crate, and `humble-client` / `steam-client` are deliberate leaf crates
with zero workspace dependencies. **B pays more to protect the sink; A pays less and removes the
thing that needed protecting.**

### C. Third point fix + a comment

**Rejected**, explicitly, by #173. This is the option whose failure mode is already documented twice.

---

## Design

### Mechanism 1 — seal `humble-client`

```rust
// crates/humble-client/src/lib.rs
#[error("network error talking to humble: {0}")]
Network(String),          // was: Network(#[from] wreq::Error)

/// Strip the request URL before stringifying. Order fetches embed the gamekey as
/// `/api/v1/order/{gamekey}`, and `wreq::Error`'s Display appends the URL (error.rs:229-230)
/// while its Debug prints the `url` field (error.rs:198-199). `without_url()` (error.rs:77)
/// drops the url and keeps the kind — the half that has diagnostic value.
fn net(e: wreq::Error) -> HumbleError { HumbleError::Network(e.without_url().to_string()) }
```

Every site that relied on `?` converting a `wreq::Error` becomes `.map_err(net)?`. The compiler
enumerates them; no grep is trusted to.

### Mechanism 2 — the census that notices, rather than classifies

A test in the workspace that asserts two things.

**(a) A `UrlBearing` error type may be NAMED only where it is being sealed.** Not "no enum stores
one" — that was this spec's first draft and it is too narrow. It checks the variant and leaves
`tracing::warn!(error = ?raw_client_err)` at a log site wide open, which is *the exact shape #171
fixed in `deliver()`*. So the check is on the type name itself: every occurrence of a `UrlBearing`
crate's error path (`wreq::Error`, `reqwest::Error`, …) across `crates/*/src/**/*.rs` must fall inside
a **pinned allow-list of sealing sites** — today the two `fn net(…)` conversions and `deliver()`'s
`%e.without_url()`. Any new occurrence anywhere, in a variant or at a log site or in a helper nobody
has written yet, fails the census and has to be either sealed or added to the allow-list by a human.

This is deliberately syntactic — but **"you cannot render a value of a type you cannot name" is FALSE,
and it was this spec's load-bearing sentence until it got measured.** Type inference names nothing:
`http.post(url).send().await` binds a `reqwest::Error` in an `Err(e)` arm without the string
`reqwest::Error` appearing anywhere. `deliver()` is exactly that shape. **A name-based census cannot
see it.** So the argument is not "unnameable ⇒ unreachable"; it has to be built on where such a value
can be *minted*:

> A raw client error can only come into existence at a **verb** — `.send()`, `.bytes()`, `.text()`,
> `.json()` — inside a crate that directly depends on the client. That set is small, enumerable, and
> **countable without type inference.**

Measured 2026-08-10: `humble-client` 21 verbs (9 `.send()`, 12 `.bytes()`), `steam-client` 13,
`fulfillment` 3 (of which one, `deliver()`, is the only real error boundary). So the census pins
**verb counts per producer crate** against a reviewed table alongside the name allow-list. A new HTTP
call anywhere in a producer crate moves a count and fails the census, which is precisely the event
that needs a human — *"you added a network call; seal its error."*

The erasure hatches that would defeat both halves (`anyhow` / `eyre` / `Box<dyn Error>`) are measured
absent from the workspace (below), so there is no third way for such a value to arrive.

**And the checker does not strip comments — measured decision, not laziness.** The whole population
today is 5 occurrences and **3 of them are prose** (`operator_message.rs:15`, `lib.rs:4525`,
`steam-client:938` all *discuss* the leak). A comment-stripper is the kind of parsing that can produce
a false *negative*, and a census whose failure mode is silence is the defect this arc exists to
remove. So the allow-list is `(file, line-content, reason)` for **every** occurrence including the
prose, which costs five lines and makes the list double as an index of every place in the workspace
where this class is reasoned about. Post-change the list loses `humble-client:264` and gains its
`fn net`.

**(b) The pinned table still describes reality.** The population is **the union of `[dependencies]`
across the 7 workspace crates** — 30 entries today, 25 of them foreign. The test compares that union
against a committed table where every foreign dependency carries an explicit verdict:

| verdict | meaning |
|---|---|
| `UrlBearing` | its error type can render a request URL; must never be stored, only converted (`reqwest`, `wreq`) |
| `NoErrorType` | exposes no error type we hold |
| `ReviewedSafe` | error type reviewed; cannot carry a URL or credential |

**Any dependency absent from the table fails the test**, with a message naming the new crate and what
decision is required. A dependency *removed* from the workspace also fails, so the table cannot rot
in the other direction either.

#### Why this census is complete, and exactly where it is not

**Complete:** a workspace error enum can only *store* a type it can *name*, and it can only name a
type from a direct dependency. Measured on 2026-08-10: the workspace has **zero `anyhow`, zero
`eyre`, zero `Box<dyn Error>`** — there is no erasure hatch through which a transitive crate's error
could arrive unnamed. So "review every direct dependency" is not a heuristic that hopes to catch
clients; it is a *provably exhaustive* population for the stated property.

**Not complete against:** a direct dependency that **re-exports** a foreign error type
(`dep::SomeOtherCrateError`). The table's verdict is then a claim about the re-export too, and a
reviewer must say so. This gap is named in the test's own doc comment rather than left for someone to
discover — **the guard states its own blind spot, because a census inherits the blind spot of its
pattern.**

#### The census must be shown to fail

A detector with no labelled positive is not a detector. The test module ships with a **negative
control**: a fixture representing a synthetic `Network(#[from] wreq::Error)` variant and an unknown
dependency, each asserted to be *rejected*. If the checker is ever refactored into something that
cannot fail, these fail first.

### Mechanism 3 — correct the doctrine where it lives

`operator_message.rs`'s header is the file a future author reads before touching either sink. It gets
one added paragraph: the CloudWatch side now has a mechanism too, what that mechanism is, and where
the census lives. **The file that once told a reader the leak was fine must be the file that tells
them it is sealed.**

---

## Non-goals

- **Not** converting the 49 `error = ?e` sites. Once no unsealed value exists, they are safe as
  written, and 49 mechanical edits would bury the two lines that carry the guarantee.
- **Not** collapsing `steam-client`'s 13 verbs onto a helper in this PR — even though the measurement
  above says it needs it. That is a second crate's worth of mechanical churn riding a PR whose subject
  is the mechanism, and it would make the diff argue for itself less clearly. **It gets its own issue,
  filed with the measurement**, and the census's reviewed count for `steam-client` is pinned at
  today's `13` with that issue named in the table — so the guard records the debt instead of implying
  it away. `steam-client`'s errors are `Network(String)` already, so nothing *stored* is unsealed;
  what is unforced is the conversion at its verbs.
- **Not** #151's ping-formatter work or #172's `ReadFailed` decisions. Both are the same *family*;
  neither is this *property*. They keep their issues.
- **Not** redacting workspace error types' own payloads (gamekeys in `Api(u16)`-adjacent messages,
  claim ids). Those are deliberate diagnostics on the access-controlled side, and #173 established
  the gamekey is already logged beside `claim_id`.
- **No** terraform, no new AWS resources, no wire-path change.

## Testing

- `cargo test -p humble-client` — existing suite must pass unchanged; the variant's `Display` text is
  deliberately identical, so no test that asserts on the message needs editing.
- New census test + its two negative controls.
- **This box cannot link `cargo test`** (needs ≥1638M, measured). Local gate is
  `cargo check --workspace --all-targets` + `clippy -- -D warnings`; **test evidence comes from
  GitHub CI only**, and per #185's still-open `--no-fail-fast` gap, a green run must be confirmed by
  finding the *named test binaries* in the log rather than by the check being green.

## Risks

| risk | mitigation |
|---|---|
| `without_url()` drops diagnostics an operator wants | It keeps the error *kind*. The dropped gamekey is already logged beside `claim_id` at `fulfillment:1077`. **Open question for review.** |
| Removing `#[from]` produces a large mechanical diff | The compiler enumerates the sites exactly; scope is measured before the plan is written, not estimated. |
| The census test is brittle in CI (parsing Rust with regex) | It parses for a narrow, committed shape and ships negative controls that prove it still rejects. A brittle *failing* census is safe; a brittle *passing* one is the defect — so every check is written to fail closed. |

## Open questions for OMBB / Lilith

1. **Is `without_url()` the right trade at the Humble boundary**, or does the gamekey-in-URL carry
   enough diagnostic value that a redacted-but-correlatable form is better?
2. **Is "every direct dependency gets a verdict" the right population**, or does the re-export gap
   need closing now rather than documenting?
