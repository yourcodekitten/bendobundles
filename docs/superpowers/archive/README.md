# archive — completed SDD docs

Design specs and implementation plans whose work has **shipped and been verified live**. Kept for
history; not active planning surface. Active (in-flight or with open follow-up) docs live one level
up in `docs/superpowers/plans/` and `docs/superpowers/specs/`.

Archived 2026-07-06:

| doc | shipped as | evidence |
|-----|-----------|----------|
| `specs/2026-07-02-bendobundles-design.md` | the whole app | deployed live; realized end-to-end |
| `plans/2026-07-02-plan1-backend-core.md` | `crates/domain,humble-client,dynamo` | present + live-validated |
| `plans/2026-07-02-plan2-lambdas.md` | `crates/fulfillment,public-api,admin-api` | present + deployed |
| `plans/2026-07-03-plan3-frontend.md` | `web/` SPA | present + deployed |
| `plans/2026-07-03-plan4-terraform.md` | `terraform/*.tf` stack | PR #4 deployed live |

**Not archived:** `specs/2026-07-05-humble-choice-design.md` — the Choice feature shipped
(PRs #24–#41, live 2026-07-06), but that spec still reads `Status: proposed` and names remaining
follow-up (the 2019–2021 bulk-claim backlog). It stays active until closed out.
