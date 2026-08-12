# Green must mean green: adapter coverage + a complete failure census (#186 · #185 · #168)

**Status:** family-reviewed — OMBB GO-with-corrections + Lilith conditional-yes, both integrated
(rev 2, 2026-08-12T07:4x-04:00). Blockers D5-1/D6-1 fixed; every correction credited inline.
**Date:** 2026-08-12 · **Author:** code kitten
**Issues:** #186 (core) · #185 (rider) · #168 (capture-or-close)

## The problem, in one paragraph

Every API in this app has two halves: the axum `Router` (109 tests, genuinely well covered) and
the event-translation layer (`lambda_http`) that turns an API-Gateway event into the
`http::Request` the Router sees. The second half appears in three lines of the repo
(`admin-api/src/main.rs:134`, `public-api/src/main.rs:59`, `fulfillment/src/main.rs:135`) and
**zero tests** — two major-version bumps of it shipped in #184 on the strength of "it compiles,"
and its failure mode is TOTAL: if path derivation changes, every route 404s at once, in
production, with green CI behind it. Meanwhile the CI test run is fail-fast (#185): when two
crates break for the same reason the log shows one and gives no sign the other was never
measured — paid for live in #180, where a second full CI cycle was burned and the second red
read as a new regression. And #168 records intermittent `api_test` failures with no captured
output. One theme: **a green CI must mean what it appears to mean.**

## Corrections to #186's own text (measured, not assumed)

1. **The issue says "canned API-Gateway v2 proxy event." Production is REST (v1), payload
   format 1.0.** `terraform/aws-apigateway.tf` uses an OpenAPI body with `aws_proxy`
   integrations on a REST API; there is no `apigatewayv2` resource and no
   `payload_format_version` attribute in the repo — REST proxy is always 1.0. A v2 fixture
   would exercise a deserializer arm production never takes. **Fixtures are v1**, with one v2
   fixture and one ALB fixture kept only as discriminator guards (see D4).
2. **Stage handling is not "API GW strips it."** The event carries `path` stage-less but
   `requestContext.stage = "live"`, and `lambda_http` *re-prepends* the stage unless
   `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` is set — which terraform sets on both API lambdas
   (`terraform/aws-lambda.tf:89`, `:132`). The archived plan note
   (`docs/superpowers/archive/plans/2026-07-03-plan4-terraform.md:852`) attributes the
   stripping to API GW and does not mention the flag; the terraform comment is the accurate
   account. (Archives stay unedited; this spec is the correction of record.)
3. **The flag is PRESENCE-triggered, not value-triggered** (OMBB + Lilith D5-4, independently;
   verified at `lambda_http-1.3.0/src/request.rs:408`): `env::var(...).is_ok()` — any value
   activates it, including `"false"` and `""`. Terraform's `= "true"` works by presence, not
   truth. Consequences: the env-free control requires the var *absent*, never falsy; and an
   operator "disabling" the flag with `false` would silently change nothing — D3 pins this
   with a dedicated test binary.

## Goals

- **G1 (#186):** integration tests that feed checked-in, v1-shaped API-Gateway proxy-event JSON
  through the REAL `lambda_http` translation into the same `router(...)` constructors the
  existing tests already use — proving event→`http::Request` derivation (path + stage flag,
  query value multiplicity, `Cookie` header carriage, base64 body decode) lands on the intended
  handler with the intended values.
- **G2 (#186, response side):** at least one test through `lambda_http::Adapter::from(router)`
  asserting the **v1 response shape** — headers (including `Set-Cookie` for the admin session)
  land in `multiValueHeaders`, `cookies` (the v2 location) stays absent, and `isBase64Encoded`
  is asserted — because the admin session cookie's fate is origin-dependent and currently
  unobserved.
- **G3 (#185):** `cargo test --workspace --no-fail-fast` in `.github/workflows/ci.yml:36`.
  Own commit, referencing the #180 incident. (One functional line; an explanatory comment
  rides with it.)
- **G4 (#168):** resolve the flake report per its own bar. The `set_var` hypothesis is dead
  (zero occurrences in `crates/`). The strongest candidate mechanisms found by inspection:
  (a) `create_table_for_tests`' waiter-less delete→create against the one shared
  dynamodb-local instance (`crates/dynamo/src/lib.rs:2895-2948` — delete result discarded,
  no DELETING-drain, no ACTIVE wait, under `--workspace` parallelism); (b) **two uuid-minting
  call sites** (`admin api_test.rs:1299`, `:1598`) that mint a fresh uuid-named table **per
  call** — ~19+ tables per full run across their ~13+6 callers (OMBB's recount; my "two
  tables" was a ~10× undercount) — never dropped, growing the shared instance forever.
  Per-test-stable names make *cross-test* collision impossible; the race is same-name across
  RUNS plus load on the shared local. Harden both; close #168 claiming the weaker thing,
  verbatim: *"not reproduced; two real defects found by inspection and hardened; reopen on
  captured output."* Never let the close read as a diagnosis (Lilith's phrasing, adopted).

## Non-goals

- No AWS, no network in tests — fixtures + the adapter as a `tower::Service` only. (The one
  permitted out-of-band AWS touch is the D5 provenance *capture attempt*, which never enters
  any test.)
- No production code changes. Test code, test helpers (`create_table_for_tests` is
  test-support inside `dynamo`), fixtures, and one CI workflow line. **One manifest exception**
  (plan-review B1): `crates/dynamo/Cargo.toml` gains `tokio` in `[dependencies]` with the
  `time` feature — the waiter needs `tokio::time::sleep`, dynamo currently has tokio only in
  dev-deps, and the feature is declared explicitly rather than trusted to transitive
  unification. Weight-free at runtime: every consumer binary already carries tokio.
- fulfillment's `lambda_runtime` path: its handler takes `LambdaEvent<serde_json::Value>` and
  dispatches by shape — already covered end-to-end by `handler_test.rs`; there is no HTTP
  translation layer to test. Named here so its exclusion is a decision, not an oversight.
- #161/#162/#163 stay their own issues. Archived docs stay unedited.
- **Deliberate narrowing (plan-review M5, recorded rather than papered over):** the
  multi-value-query test asserts translation on the `Request` itself (RequestExt + the
  reconstructed URI) and does NOT drive the steam-return route's
  `Query<Vec<(String,String)>>` extractor (`public-api/src/lib.rs:342`) — that route's
  behavior depends on Steam config absent from adapter tests, and extractor consumption of a
  well-formed query is axum's contract, already exercised by the direct-router suites. The
  translation property is fully asserted; the extractor hop is not re-proven here.

## Design

**D1 — placement.** Dedicated test binaries per concern (see D2 for why binaries are the unit
of env isolation): `crates/public-api/tests/adapter_test.rs`,
`crates/public-api/tests/adapter_stage_control_test.rs`,
`crates/public-api/tests/adapter_stage_false_test.rs`,
`crates/admin-api/tests/adapter_test.rs`. Fixtures at `crates/<crate>/tests/fixtures/*.json`
(the repo's established idiom). `lambda_http` is already a normal dependency of both API
crates. Separate binaries keep every stage-env decision away from the existing 109 tests'
isolation profile. Cost noted: four new link steps in CI; the box does not link test binaries
locally (measured 2026-08-07: a `cargo test` link needed ≥1638M and OOM'd the box — banked
measurement with a date, not an eternal truth; CI is the linker of record).

**D2 — the stage env var, the sharp edge.** Reproducing production means
`AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH` present in the test process, and edition-2024
`set_var` is `unsafe` + racy. Chosen route: in each flag-bearing binary, a `std::sync::Once`
init does `unsafe { set_var }` **before any translation**, every test calls the init helper
first, and the file header states why the binary is separate and that the flag is
presence-triggered. **The decisive argument against `.cargo/config.toml [env]`** (OMBB —
my draft argued this wrong): the repo already uses `[env]` for `DYNAMODB_LOCAL_URL`, so
"ambient is un-idiomatic" was false; the real killer is that **`[env]` can set but never
unset**, and the flag-ABSENT control arm requires the var truly absent — impossible under a
workspace-wide `[env]` entry. Three binaries, three env worlds:
- `adapter_test.rs` (× both crates): flag **present** (`"true"`) — production's world.
- `adapter_stage_control_test.rs`: flag **absent** — proves the flag is load-bearing (stage
  gets re-prepended and routing breaks). Opens with a self-checking precondition: assert the
  var `is_err()`, with a message saying the control is VOID (not failed) if ambient env ever
  leaks in — a control that can't distinguish "flag absent" from "nobody checked" is the
  defect D6 exists to kill. Also carries the stage-ABSENT/`$default` fixture arm
  (plan-review M4): with no stage in the event, the path is not prefixed even without the
  flag (`request.rs` `None => path.into()`).
- `adapter_stage_false_test.rs`: flag **present with value `"false"`** — pins the presence
  footgun (still strips). This is the test an operator's mental model needs.

**D3 — request-side tests (G1), per crate.**
- public-api: `GET /api/l/{token}` unknown token → the app's own JSON 404 from the matched
  route (`{"error":"unknown link"}`, `lib.rs:1082`) vs an unmatched path → the router
  fallback's `{"error":"not found"}` (`lib.rs:189-195`) — **the two-shape 404 pair**: path
  derivation pinned from both sides with no handler instrumentation. Plus a multi-value query
  fixture (value multiplicity via RequestExt + reconstructed URI; ordering is NOT asserted —
  the reconstruction is map-backed and order is not part of the property; recorded as a
  D6-4-style demotion, "asserted" not "pinned"), and a base64-body POST fixture whose decoded
  bytes are asserted exactly and whose parse must clear the Json extractor.
- admin-api: `POST /admin/api/links/{token}/note` with a real minted session — session minted
  via the already-covered direct-router login, then carried through translation in a fixture
  whose `Cookie` and `X-Admin-Request` headers appear in **both** `headers` and
  `multiValueHeaders` (real REST events populate both maps, always — OMBB Q4; translation
  merges them, and a fixture populating only one would exercise a merge arm production never
  takes alone).
- both: the stage-flag pair across D2's binaries — flag present → route resolves; flag absent
  → translated path is `/live/...` and routing breaks. Lilith's review names this the house
  standard for a two-direction control; D6 raises the rest of the suite to it.

**D4 — discriminator guards.** The deserializer is a trial cascade with v1 first and
`pass_through` OFF (`deserializer.rs:29-34`). Two guard fixtures pin the cascade:
- **v2 guard:** must parse **as v2** (asserted via the parsed `RequestContext` variant — which
  simultaneously asserts it did NOT get captured by the v1 arm; "parses at all" alone is
  near-vacuous since v2 requires only a v2-parseable `requestContext`, Lilith D6-6).
- **ALB guard** (OMBB Q4): one ALB fixture asserting the ALB arm — the cascade has more than
  two arms and a two-arm guard leaves the rest of the fall-through class unpinned.
Sabotage recipes for both are specified in the plan (D6 requires every guard demonstrate its
red; a guard that has never screamed never guarded).

**D5 — fixtures earn the word "real" (rewritten after Lilith's D5-1, which caught my own
correction-class one level down).**
- **The authority chain, stated honestly:** fixtures are built to the **documented full REST
  proxy event shape + this repo's terraform config** — NOT "derived from lambda_http's test
  corpus." The corpus (`lambda_http-1.3.0/tests/data/apigw_proxy_request.json`) is a
  **parser-exercise artifact**: eight top-level keys, no `multiValueHeaders`, no
  `multiValueQueryStringParameters`, no `body`, no `isBase64Encoded`, no
  `requestContext.path`. It bites mechanically: v1 translation **prefers the multi-value
  query map** and falls back to single-value only when it's empty — production always sends
  both maps, so a corpus-minimal fixture exercises the fallback arm production never takes.
  The corpus is authority for *"parses"*, never for *"is what AWS sends."*
- **"Real-shaped" is a predicate, not an adjective:** every v1 fixture is loaded through a
  `load_v1_fixture()` helper that asserts (i) the **full production key-set** is present
  (both header maps, both query maps, `body`, `isBase64Encoded`, `requestContext.path`,
  `requestContext.stage`, ...), and (ii) **correlated-field consistency** —
  `requestContext.path == "/" + stage + path`, top-level `httpMethod ==
  requestContext.httpMethod`, `resource == requestContext.resourcePath` — so every future
  hand edit is checked by machine, closing the class and not the instance (Lilith D5-2).
- **Capture attempt (upgrade, not dependency):** during execution, attempt ONE real-event
  capture via API Gateway test-invoke with the kitten roles. If permitted: diff the captured
  key set against the loader's list and bank the receipt — which then claims exactly
  *"gateway-verified key set; the CloudFront layer is still asserted-not-captured"*
  (test-invoke enters at the gateway and never crosses CloudFront — Lilith). If denied: the
  documented-shape authority stands and the receipt says so. Tests stay no-AWS either way.
- **No `_provenance` field, for the right reason:** my draft claimed strict event JSON
  forbids it — false; `aws_lambda_events` 1.2.0 has zero `deny_unknown_fields` and unknown
  fields deserialize fine (verified). The real rule: **a fixture claiming to be real-shaped
  must not carry a field production never sends** — same law as the rest of D5. Provenance
  lives in each test binary's header doc comment (the sidecar idea is dropped; one home).

**D6 — sabotage controls: the red census is an artifact (raised to Lilith's contract).**
A test whose red state has never been observed is a comment. The plan's census (batched
sabotage commit → one CI run → revert) must produce, per **property** (not per test —
a five-assertion test red once may carry four assertions that never fired):
1. the exact sabotage edit,
2. **proof the sabotage landed** — the diff touched the intended field and only it (a
   mis-aimed edit that matches nothing leaves green; one that lands differently reds a
   stranger),
3. the observed failure line, which must be **the specific assertion the test exists to
   make** — a red from an upstream panic or parse error proves nothing about the routing
   assertion,
recorded in the PR description as a table (session evidence is not evidence; the PR is the
record). Response-side included: the login fixture's password is sabotaged so no `Set-Cookie`
is minted and the `multiValueHeaders` extraction assert itself fires (input-side form —
editing the expected value only proves the assert executes). Expected-failure lists are
**exact and bidirectional**: every listed test must fail, every unlisted test must stay
green. The stop-rule on an unexpected green is *diagnose which of test/sabotage is broken*
— two of my own draft recipes were verified no-ops in plan review (the sabotage, not the
test, was the defect), so "fix the test" as a reflex would have mutilated correct tests.

**D7 — the hidden-seam risk, scoped honestly (OMBB Q2).** BOTH halves of the adapter tests
ride `#[doc(hidden)]` API: request-side `lambda_http::request::LambdaRequest` +
`RequestOrigin`, response-side `Adapter` (public via the `From` impl) + `LambdaResponse`.
There is **no other offline seam** — `from_response` is `pub(crate)`, `IntoResponse` stops
before v1 serialization (OMBB, measured on 1.3.0). Accepted with eyes open: clearly
commented, cheap to rewrite if 2.x reshapes it, and the alternative is the status quo —
zero observation of the most production-relevant response behavior. Response assertions are
made on the **serialized JSON** of the response, not the hidden enum's shape, to minimize
coupling.

## Acceptance criteria

1. Path/stage (present, absent, `$default`, and `"false"`), query value-multiplicity, cookie
   (both maps), and base64 translation each pinned by at least one v1 fixture test whose
   **specific assertion** has been observed red under a landed sabotage (D6) and green on
   main. Properties asserted but not sabotage-pinned (query ordering) are named as such,
   not counted.
2. One response-side test proves the admin `Set-Cookie` lands in `multiValueHeaders`, the
   v2-style `cookies` array stays absent, and `isBase64Encoded` is false — observed red via
   the sabotaged-login form.
3. `ci.yml` runs `cargo test --workspace --no-fail-fast`; the diff is one functional line
   plus its comment, nothing else.
4. Every v1 fixture passes the `load_v1_fixture()` key-set + consistency predicates — and
   the predicate earns its red **per arm**: one deliberately-degenerate fixture per defect
   class (key-set; correlated-field), each with its own `#[should_panic]` expecting that
   arm's message. One planted defect per copy — a both-defects fixture proves only the arm
   that panics first, masking the other (Lilith's required edit; the 08-06 house classic).
5. `create_table_for_tests` drains DELETING and waits bounded-time for ACTIVE (table + GSIs);
   the two uuid-per-call sites use stable names; both changes are test-support-only (plus
   the one declared manifest line).
6. #168 closed with the exact wording of G4 — *not reproduced; hardened; reopen on captured
   output* — plus the mechanism inventory and commits.
7. All existing tests untouched and green; the 109 existing API tests' isolation profile
   unchanged (no ambient env added to their binaries).

## Family review — answered questions (record, 2026-08-12)

- **Q1 (D2):** separate binaries — CONFIRMED, but for the right reason now: `[env]` cannot
  unset, and the control arm needs true absence. (OMBB; my draft's "ambient is icky" was a
  wrong argument for a right choice.)
- **Q2 (D7):** yes, take the hidden seam — there is no other offline seam, and the risk
  spans both halves, now scoped as such. (OMBB, measured.)
- **Q3 (G4):** close #168 claiming the weaker thing; exceeds the issue's own bar as long as
  the close never reads as a diagnosis. (OMBB + Lilith, converging.)
- **Q4:** three additions adopted — the `"false"` presence binary, the ALB guard, both-maps
  cookie population. Error-Display leak class checked and already sealed (#189). (OMBB.)
- Lilith's full D5/D6 review (blockers D5-1, D6-1; ambers D5-2/3/4, D6-2..6) — all
  integrated above; her review file is archived in my state inbox and its substance is this
  spec's rev-2 diff.
