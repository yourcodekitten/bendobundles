# Per-lambda DynamoDB policies (#70)

Everything in this directory except `simulate.sh` and this README is **generated** by
`crates/dynamo/tests/iam_capture.rs` — do not hand-edit. The generator drives every
`Store` method exactly as each lambda calls it (an SDK interceptor logs the serialized
requests) and derives the policy documents from that traffic, so the attribute allowlists
and deny prefixes are captured, never hand-enumerated.

```
IAM_CORPUS_WRITE=1 cargo test -p dynamo --test iam_capture   # regenerate after code changes
cargo test -p dynamo --test iam_capture                      # CI drift gate: files == code
terraform/policies/simulate.sh                               # policy-engine proof (needs iam:SimulateCustomPolicy)
```

The same test **without** the env var asserts the committed files match the code — a new
or widened store call fails CI instead of 403ing in prod after the policy ships. Review a
corpus diff like the IAM change it is. (Known future example: #134's claim-tx version
counter will widen the `GAME#` scoped-update allowlist by one attribute — the gate will
demand the regeneration.)

## What each lambda gets

| lambda | policy | shape |
|---|---|---|
| public-api | `dynamo-rw-public.json.tpl` | no `Scan`; `UpdateItem` only via per-prefix scoped-writer statements (below); `Deny SESSION#*`/`SYNC#*` (#84) |
| admin-api | `dynamo-rw-admin.json.tpl` | full captured surface (owns `SESSION#`, Scans `list_all_games`/`list_links`); `Deny OIDCSTATE#*` |
| fulfillment | `dynamo-rw-fulfillment.json.tpl` | captured surface (owns `SYNC#`); `Deny SESSION#*` + `OIDCSTATE#*` |

After #70, **no lambda except admin-api can read or mint an admin session token**, which
was the strongest argument for this work (issue #70 discussion): sessions share the table
with every `LINK#`/`CLAIM#` item, and `session_middleware` admits on expiry alone.

## The scoped-writer statements (public-api)

Public's `UpdateItem` is split out of the broad allow into two statements conditioned on
`ForAllValues:StringEquals dynamodb:Attributes` — the union of names its two real update
paths utter (`claim_game`'s transact updates; `set_link_thanks`), keys and
condition-expression names included, straight from the corpus:

- `ScopedUpdateGAME` (`GAME#*`): `pk, sk, body, status, claim_id, gsi1pk, gsi1sk, version` (#134: the claim tx bumps the game's version counter)
- `ScopedUpdateLINK` (`LINK#*`): `pk, sk, body, claims_used, claims_allowed, expires_at, revoked, thank_note, thanked_at`

Both pin `dynamodb:ReturnValues` to `NONE | UPDATED_*` (`StringEqualsIfExists`) so a
compromised public-api can't turn `UpdateItem` + `ReturnValues=ALL_OLD` into a full-item
read of attrs outside its allowlist.

**Honest boundary of the guarantee:** `dynamodb:Attributes` cannot distinguish a
ConditionExpression read from a `SET`. Names the legit transact merely *conditions on*
(`revoked`, `claims_allowed`, `expires_at`) are therefore in the allowlist and remain
IAM-writable by a hypothetically compromised public-api. The structural wins are the names
public traffic never utters at the top level: `gift_note` on links, and the entire game
enforcer family (`hidden`, `hidden_source`, `owned_by_ben`, `steam_app_id`,
`appid_source`) — all unwritable regardless of any future public-api bug, which is
exactly the app-layer scoped-writer guarantee lifted into IAM. (`body` stays writable —
the enforcer pattern already treats top-level attrs as authoritative over `body` on read
for precisely this class of reason.)

Deliberately **no** `Attributes` conditions on reads: the code sends no
`ProjectionExpression`/`Select`, the context key is absent on such reads, and the AWS
pattern would require `Select=SPECIFIC_ATTRIBUTES` conditions that 403 every real
`GetItem`. Write-side is where the guarantee lives. Similarly none on admin/fulfillment
writers: `update_link_meta` and the sync/compensate paths legitimately span the whole
attribute alphabet — a full-alphabet allowlist is theater.

## Design notes carried from #84

- The `Deny` statements use `ForAnyValue:StringLike dynamodb:LeadingKeys` on the **base
  table only**: a GSI query's `LeadingKeys` is the *index* key, so `listable` /
  `pending-claims` queries never match the deny — this is why the denies are GSI-safe
  where a LeadingKeys *allowlist* would not be.
- Public keeps no `Scan` anywhere (and none was captured — asserted in the generator):
  `LeadingKeys` cannot constrain a `Scan`, so any Scan grant would make every deny theater.
- All three keep `ConditionCheckItem` defensively. TransactWriteItems is documented to
  authorize per-element as the underlying ops; #84 shipped public with it kept, and we
  hold that defensive line uniformly rather than gamble a live 403 on doc-precision.
- `take_oidc_state` uses `ReturnValues=ALL_OLD` on `DeleteItem` — which is why the
  ReturnValues pin lives only on the scoped Update statements, not the broad allow.
- `set_link_thanks` uses `ReturnValuesOnConditionCheckFailure=ALL_OLD`; that parameter is
  not the `dynamodb:ReturnValues` context key and is not gated — accepted residual (it
  returns an item public may read anyway).

## Proof

`simulate.sh` replays every corpus shape through `aws iam simulate-custom-policy` against
the same policy bytes terraform deploys (positive matrix → `allowed`), then probes the
blast-radius requests the policies exist to stop (→ deny). The simulator is the real AWS
policy evaluation engine — same verdict authority as a live 403 with no infra touched. It
needs `iam:SimulateCustomPolicy` (read-only, evaluates hypothetical documents); the box
role currently lacks it, so the proof transcript on the PR is the reviewer-runnable
artifact until that grant exists.
