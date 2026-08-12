# Green must mean green: adapter coverage + a complete failure census (#186 · #185 · #168)

**Status:** draft — family review before build, per house pattern.
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
   fixture kept only as a discriminator guard (see D4).
2. **Stage handling is not "API GW strips it."** The event carries `path` stage-less but
   `requestContext.stage = "live"`, and `lambda_http` *re-prepends* the stage unless
   `AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true` — which terraform sets on both API lambdas
   (`terraform/aws-lambda.tf:89`, `:132`). The flag is read from process env **per request**
   (`lambda_http-1.3.0/src/request.rs:408`). The archived plan note
   (`docs/superpowers/archive/plans/2026-07-03-plan4-terraform.md:852`) attributes the
   stripping to API GW and does not mention the flag; the terraform comment is the accurate
   account. (Archives stay unedited; this spec is the correction of record.)

## Goals

- **G1 (#186):** integration tests that feed checked-in, v1-shaped API-Gateway proxy-event JSON
  through the REAL `lambda_http` translation into the same `router(...)` constructors the
  existing tests already use — proving event→`http::Request` derivation (path + stage flag,
  query incl. multi-value, `Cookie` header carriage, base64 body decode) lands on the intended
  handler with the intended values.
- **G2 (#186, response side):** at least one test through `lambda_http::Adapter::from(router)`
  asserting the **v1 response shape** — headers (including `Set-Cookie` for the admin session)
  land in `multiValueHeaders`, and `isBase64Encoded` behaves — because the admin session
  cookie's fate is origin-dependent and currently unobserved.
- **G3 (#185):** `cargo test --workspace --no-fail-fast` in `.github/workflows/ci.yml:36`.
  Own commit, referencing the #180 incident.
- **G4 (#168):** resolve the flake report per its own bar: the `set_var` hypothesis is dead
  (zero occurrences in `crates/`), and the strongest candidate mechanisms found by inspection
  are (a) `create_table_for_tests`' waiter-less delete→create against the one shared
  dynamodb-local instance (`crates/dynamo/src/lib.rs:2895-2948` — no DELETING-drain, no
  ACTIVE wait, under `--workspace` parallelism), and (b) two uuid-named tables that are minted
  per run and never dropped (`admin api_test.rs:1299`, `:1598`), growing the local instance
  forever. Harden both (bounded ACTIVE waiter; stable names or cleanup), state plainly that
  this is *probable-mechanism hardening, not a reproduced diagnosis*, and close #168 with
  reopen-on-output instructions. Whether that closure standard is acceptable is a family
  question (Q3).

## Non-goals

- No AWS, no network: fixtures + the adapter as a `tower::Service` only.
- No production code changes. Test code, test helpers (`create_table_for_tests` is
  test-support inside `dynamo`), fixtures, and one CI workflow line.
- fulfillment's `lambda_runtime` path: its handler takes `LambdaEvent<serde_json::Value>` and
  dispatches by shape — already covered end-to-end by `handler_test.rs`; there is no HTTP
  translation layer to test. Named here so its exclusion is a decision, not an oversight.
- #161/#162/#163 stay their own issues. Archived docs stay unedited.

## Design

**D1 — placement.** New test binaries `crates/public-api/tests/adapter_test.rs` and
`crates/admin-api/tests/adapter_test.rs`, fixtures at `crates/<crate>/tests/fixtures/*.json`
(the repo's established idiom). `lambda_http` is already a normal dependency of both crates —
no manifest changes for it. Separate binaries keep the stage-env decision (D2) away from the
existing 109 tests' isolation profile. Cost noted: two new link steps in CI; the box doesn't
link tests locally anyway (CI is the linker of record).

**D2 — the stage env var, the sharp edge.** Reproducing production means
`AWS_LAMBDA_HTTP_IGNORE_STAGE_IN_PATH=true` in the test process, and edition-2024 `set_var` is
`unsafe` + racy. Chosen route: in each `adapter_test.rs`, a `std::sync::Once` init that does
`unsafe { set_var } ` **before any translation**, with every test in the binary calling the
init helper first, and a file-header comment stating that this is why the binary is separate.
Rejected: `.cargo/config.toml [env]` (workspace-wide ambient config for a single binary's
concern, and silently absent when a binary is run directly). Both stage branches are ALSO
covered env-free via fixture variation (`requestContext.stage` present vs absent) so the
translation logic itself is pinned independently of the env mechanism. — Family Q1.

**D3 — request-side tests (G1), per crate.**
- public-api: `GET /api/l/{token}` unknown token → the app's own JSON 404 from the fallback
  handler (the same probe the #184 deploy used by hand, now automated); a multi-value query
  fixture through the Steam OpenID return route's `Query<Vec<(String,String)>>` extractor
  (`public-api/src/lib.rs:342`) — `build_request_uri` re-serializes the query map, so ordering
  and encoding are real assertion targets; a base64-body POST fixture.
- admin-api: `POST /admin/api/links/{token}/note` with a real minted session — reuse the
  existing `admin_login` helper to obtain the cookie, then carry it in the fixture's `Cookie`
  header (fixture templated at runtime for the session value; the JSON on disk holds a
  placeholder) plus the `X-Admin-Request` CSRF header the middleware requires.
- both: one stage-prepend regression fixture — stage `live` present; with the flag set the
  route must resolve; with the flag unset (env-free twin binary-local control, see D2) the
  translated path is `/live/...` and the router 404s — pinning that the flag is load-bearing,
  which is precisely what production config asserts.

**D4 — discriminator guard.** One v2 fixture asserting `from_str` still parses it as v2 and it
does NOT satisfy the v1 fixtures' assertions — so if anyone ever swaps the gateway type or a
default-features change reorders the deserializer, the suite says so loudly rather than
silently testing the wrong arm. (`pass_through` is off; v1 is tried first —
`lambda_http-1.3.0/src/deserializer.rs:29-34`.)

**D5 — fixtures are real-shaped with provenance.** Derived from `lambda_http`'s own shipped
test corpus (`lambda_http-1.3.0/tests/data/apigw_proxy_request.json` and siblings), edited to
this app's routes, each carrying a `//`-free JSON sidecar note (a `_provenance` field is not
possible in strict event JSON — provenance goes in the test's doc comment) naming the source
and the deployed shape it mirrors (REST v1, stage `live`, CloudFront `origin_path`).

**D6 — sabotage controls (the tests must be able to fail).** For each fixture family, the plan
includes a one-line sabotage check performed during development (wrong path → red, stage flag
absent → red on the flag-dependent test, cookie stripped → 401/403, base64 flag cleared → body
mismatch). A test whose red state has never been observed is a comment, not a check — every
new test's failure mode gets demonstrated once before the PR claims coverage.

**D7 — Adapter::from is `#[doc(hidden)]`.** G2 rides a semi-internal-but-public API
(`lambda_http-1.3.0/src/lib.rs:154-183`, constructible via the public `From` impl). Accepted
with eyes open: one test, clearly commented, cheap to rewrite if 2.x hides it differently.
— Family Q2.

## Acceptance criteria

1. Path/stage, multi-value query, cookie, and base64 translation each pinned by at least one
   v1 fixture test that has been observed red under sabotage (D6) and green on main.
2. One response-side test proves the admin `Set-Cookie` lands in `multiValueHeaders` (v1 shape).
3. `ci.yml` runs `cargo test --workspace --no-fail-fast`; the diff is one line plus nothing.
4. `create_table_for_tests` waits bounded-time for ACTIVE (and tolerates DELETING-drain);
   the two uuid-table leaks are fixed; both changes are test-support-only.
5. #168 closed with the inventory of found mechanisms, the hardening commits, and explicit
   reopen-on-captured-output instructions — or kept open if family review rejects that bar (Q3).
6. All existing tests untouched and green; the 109 existing API tests' isolation profile
   unchanged (no ambient env added to their binaries).

## Open questions for family review

- **Q1 (D2):** `Once` + `unsafe set_var` inside the dedicated binaries vs `.cargo/config.toml
  [env]` workspace-wide. I chose the contained-unsafe; is the ambient-config route actually
  safer in a way I'm discounting?
- **Q2 (D7/G2):** is one response-side test through a `#[doc(hidden)]` seam worth the churn
  risk, or should response-shape coverage wait for a lambda_http release that exposes it
  properly? My lean: take it now — the cookie's multiValueHeaders fate is the single most
  production-relevant unobserved behavior in the response half.
- **Q3 (G4):** closing #168 on probable-mechanism hardening without a reproduction: honest
  enough, or does the issue's own "reproduce or close" bar mean close-as-unreproducible and
  keep the hardening separate? (Either way the hardening ships; the question is what the
  close comment claims.)
- **Q4:** anything else in the workspace that ships on "it compiles" and belongs in this
  class? (fulfillment's dispatch-by-shape is excluded by reasoned non-goal, not forgotten.)
