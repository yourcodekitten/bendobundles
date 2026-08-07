# Plan review — operator-truth (2026-08-07)

## 1. Plan shape

Seven tasks turning one 11-line notification seam into a verified delivery path: verify status (1),
resolve config at boot (2), a containment type (3), chunking (4), structured reporting (5), migrate
~20 call sites (6), an audit counter (7). The sequence maps cleanly onto the spec and the ordering
is genuinely good — Task 1 depends on nothing and fixes the highest-value defect alone. The plan
communicates its own structure. **The defects are in the tests, not the architecture**, which is the
dangerous direction: several of them pass whether or not the code is correct.

## 2. Verdict

**Ready after fixes.** Four blockers, all mechanical; but three of them are vacuous or
self-contradicting tests, and one is a genuine unresolved conflict between two stated guarantees.

## 3. Cold-subagent walkthrough findings

- **Task 1** — executes. The dirty-side verify in Step 5 is real. No defect.
- **Task 2** — breaks at Step 4. "fix any `Deps { webhook_url: .. }` construction sites the compiler
  names" is exactly the "wire it up" class the executor cannot act on. **7 sites in 3 files.**
- **Task 3** — breaks at Step 5. The trybuild fixture references `fulfillment::operator_message::…`
  but Step 3 declares `mod operator_message;` (private). Does not compile, and the failure will look
  like the compile_fail test *passing*.
- **Task 4** — executes, but two of its four tests cannot fail.
- **Task 5** — executes.
- **Task 6** — Step 1 tells the executor to print the list and then hands it a `grep -c`.
- **Task 7** — executes.

## 4. Blockers

**B1 — Task 3, Steps 3+5: module privacy contradicts the compile_fail fixture.**
Step 3: `mod operator_message;`. Step 5 fixture: `fulfillment::operator_message::OperatorMessage`.
A private module is not reachable from an integration test, so the fixture fails to compile **for
the wrong reason** — and `trybuild` reports compile-failure as **success**. The test would pass
while asserting nothing. *(This is the vacuous-pass shape: the guard reports green because the probe
was broken.)*
**Fix:** `pub mod operator_message;` — matches the repo's existing `pub mod heal_pairs;` (lib.rs:14).
Add to Step 6: paste the trybuild `.stderr` and confirm the error names `&'static str`, not
`E0603 module is private`.

**B2 — Task 2, Step 4: unspecified fan-out.**
`Deps { … }` is constructed at **7 sites across 3 files**: `crates/fulfillment/src/lib.rs`,
`crates/fulfillment/src/main.rs`, `crates/fulfillment/tests/handler_test.rs`. Telling a cold
subagent to "fix any the compiler names" leaves it guessing the replacement value in test fixtures.
**Fix:** enumerate the files in **Files**, and give the substitution explicitly:
`webhook_url: None` → `notify: Notify::Disabled`; `webhook_url: Some(u)` → `notify:
Notify::Webhook(u)`.

**B3 — Task 4, Step 1, `chunks_are_never_empty`: the test is vacuous, and it guards the ONE
requirement with real evidence.**
For `""` and `"   "`, `chunks()` returns `Vec::new()`, so the `for` body never runs and the test
passes no matter what the implementation does. The empty-chunk guard — the only end of the length
axis with observed production failures — is **untested**.
**Fix:**
```rust
#[test]
fn empty_input_yields_no_chunks_at_all() {
    for input in ["", "   ", "\n\n"] {
        let m = OperatorMessage::literal(Box::leak(input.to_string().into_boxed_str()));
        assert!(m.chunks(PREFIX).is_empty(), "empty input must post nothing, got chunks for {input:?}");
    }
}

#[test]
fn no_emitted_chunk_is_empty_or_prefix_only() {
    let m = OperatorMessage::literal(Box::leak("y".repeat(5000).into_boxed_str()));
    let cs = m.chunks(PREFIX);
    assert!(!cs.is_empty());
    for c in &cs {
        assert!(c.len() > PREFIX.len(), "prefix-only chunk: {c:?}");
    }
}
```

**B4 — Task 6, Step 1: the command contradicts its own instruction.**
The step says *"Print the list, do not trust the count"* and the command given is
`grep -c 'cannot find function'`. This is the exact defect the design review banked four hours
earlier (*a count cannot be sanity-checked; a list can*).
**Fix:** the command is
`CARGO_BUILD_JOBS=1 cargo check -p fulfillment 2>&1 | grep -n 'cannot find function' -A2`
and the step records the printed list in the commit body.

## 5. Majors

**M1 — Task 4: the bound and the no-mid-token rule CONFLICT, and the plan silently prefers the wrong
one.** In the hard-split branch the split only happens when `cur.matches("**").count() % 2 == 0`. A
run containing an unbalanced `**` therefore **keeps appending past `budget`**, producing a chunk
over 2000 — violating the hard Discord limit, which is the requirement that actually causes a 400.
Two stated guarantees, no stated precedence.
**Fix:** state precedence in the spec and the code comment — **the 2000 bound always wins.** When a
split must land mid-token, take it and append the marker ` …`; the marker is what the "never
silently shorten" rule actually requires. Add the test:
```rust
#[test]
fn the_2000_bound_wins_over_token_integrity() {
    let body = format!("{}**{}", "a".repeat(1999), "b".repeat(1999)); // one line, unbalanced **
    let m = OperatorMessage::literal(Box::leak(body.into_boxed_str()));
    for c in m.chunks(PREFIX) { assert!(c.chars().count() <= 2000); }
}
```

**M2 — Task 4: `chunks_never_split_inside_a_bold_delimiter` is vacuous.** The fixture puts
`**bold**` on its own short line, so the line-split path places it whole inside a chunk and the
hard-split guard is never reached. It passes with the guard deleted.
**Fix:** the fixture must be a **single line longer than budget** with the `**` pair straddling the
budget offset. Verify by deleting the guard and watching the test fail (dirty-side, as Task 1 does).

**M3 — Task 6, Step 3: the drift test does not test drift.** Spec test #8 requires the id in the
operator message to equal the id in the structured log record. The plan asserts only the message
side and its own comment concedes it. A renamed or dropped log field passes.
**Fix:** capture the log. Add `tracing-test = "0.2"` to dev-dependencies and assert with
`#[traced_test]` + `logs_assert` that a record exists carrying the same `req_id`. If that dependency
is unwanted, make `ping_msg` return the `req_id`s it emitted and assert against them — but do not
ship the half-test.

**M4 — spec test coverage gaps.** Three spec tests have no task:
- #2 (`NOTIFY_DISABLED` constructs, logs once, **sends nothing**) — Task 2 tests construction only.
- #4 (transport error is a failure) — Task 1 tests non-2xx only.
- #10 (`ping` still infallible: a dead webhook leaves `handle`'s outcome unchanged) — **nothing
  tests the guarantee the whole design is built to preserve.**
**Fix:** add #2 and #4 to Task 2 and Task 1 respectively; add #10 as Task 8, pinning the property
against future work.

## 6. Minors

- **m1 — Task 1 vs Task 5:** Task 1 ships `eprintln!`, Task 5 replaces it. Correct as evolution, but
  Task 1's commit body should say the structured line lands in Task 5 so a reviewer of Task 1 alone
  does not flag it as missed.
- **m2 — Task 5:** `ping_chunks_for_test` calls `Box::leak` per invocation — an unbounded leak in a
  `pub` (if hidden) API. Acceptable in tests; add `#[cfg(test)]`-adjacent wording or take
  `&'static str`.
- **m3 — Task 4:** `LABEL_RESERVE = 10` fits ` (999/999)` exactly. Fine, but state the assumption.

## 7. Interface contract table

| Task | Consumes | Produced by | Status |
|---|---|---|---|
| 1 | — | — | OK |
| 2 | `deliver` | Task 1 | OK |
| 3 | — | — | OK |
| 4 | `OperatorMessage` | Task 3 | OK |
| 5 | `deliver` (T1), `chunks` (T4), `Notify` (T2) | 1, 2, 4 | OK — no forward refs |
| 6 | `ping_msg`, `OperatorMessage`, `ErrorSummary`, `Part` | 3, 5 | OK |
| 7 | — | — | OK |

`Produces` that are never consumed: `ping_for_test` (T1) and `ping_chunks_for_test` (T5) are
test-only entry points — intentional, not dead code. **No name drift found across boundaries.**

## 8. Open questions

- **Q1:** Task 2 exits the process on unresolvable config. In Lambda this fails the init and the
  invocation retries. Is a hard fail the intended behaviour for a *notification* dependency, given
  the design's own principle that notification must never break fulfilment? **This is the one place
  the plan may contradict the spec's central guarantee.** It needs an explicit answer, not a default.

## 9. What would change the verdict

Fix B1–B4, M1–M4. B3, M1 and M2 are the ones that matter: **three of Task 4's four tests currently
cannot fail**, and the guarantee with real production evidence behind it is the one left untested.
