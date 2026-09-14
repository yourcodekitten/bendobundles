//! The scrapbook 📖 — ben-facing composition of fifteen years of giving
//! (docs/spec-scrapbook.md). Pure and clock-injected: every filter is applied
//! HERE, server-side, so the payload stays thin and the client can never
//! mis-apply `is_listable` / seal / staleness rules it cannot see.

use domain::{Claim, ClaimState, Friend, Game, Link, SELF_LINK_TOKEN};
use serde::Serialize;
use std::collections::HashMap;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// A Pending older than this is a fulfillment defect, not a gift mid-open
/// (docs/spec-scrapbook.md "claim states on the page"; the 70-day prod
/// specimen is why this bound exists). Provisional and layout-invariant.
pub const STALE_PENDING_HOURS: i64 = 48;

#[derive(Serialize, Clone)]
pub struct ScrapbookGame {
    pub id: String,
    pub title: String,
    pub artwork_url: Option<String>,
    pub acquired_at: Option<String>,
}

#[derive(Serialize)]
pub struct ScrapbookEntry {
    pub claimed_at: String,
    pub state: ClaimState,
    pub game: ScrapbookGame,
    pub recipient: String,
    pub gift_note: Option<String>,
    pub tag: Option<String>,
    pub thank_note: Option<String>,
    pub thanked_at: Option<String>,
    pub link_token: String,
    pub link_label: String,
}

#[derive(Serialize)]
pub struct ScrapbookWaitingLink {
    pub link_token: String,
    pub link_label: String,
    pub recipient: String,
    /// RFC3339 unlock moment, Some iff the link is sealed AT `now` — the one
    /// forward-pointing field on the page ("wrapped until ⟨date⟩", spec Q1 ruling).
    pub sealed_until: Option<String>,
    pub games: Vec<ScrapbookGame>,
}

#[derive(Serialize)]
pub struct ScrapbookDoor {
    pub link_token: String,
    pub link_label: String,
    pub recipient: String,
    pub claims_left: u32,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ScrapbookView {
    pub entries: Vec<ScrapbookEntry>,
    pub waiting: Vec<ScrapbookWaitingLink>,
    pub doors_open: Vec<ScrapbookDoor>,
    pub orphan_claim_count: u32,
    pub stale_pending_count: u32,
}

fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&Rfc3339).unwrap_or_default()
}

fn scrapbook_game(id: &str, games: &HashMap<String, Game>) -> ScrapbookGame {
    match games.get(id) {
        Some(g) => ScrapbookGame {
            id: g.id.clone(),
            title: g.title.clone(),
            artwork_url: g.artwork_url.clone(),
            acquired_at: g.acquired_at.map(rfc3339),
        },
        // degradation, not lying: a missing game record renders by id
        None => ScrapbookGame {
            id: id.into(),
            title: id.into(),
            artwork_url: None,
            acquired_at: None,
        },
    }
}

/// TWO predicates, deliberately — the seal splits them (spec Q1 ruling, four
/// rounds of crossfire convergence): a curated sealed link's games are
/// chosen-and-waiting (the friend just can't open yet); a sealed uncurated
/// link is NOT an open door — it is wrapped. NO precedence knowledge lives
/// here: `Sealed` outranks Expired AND Exhausted inside `can_claim`, so any
/// consumer hand-tolerating `Err(Sealed)` re-derives an ordering documented
/// three crates away (a sealed EXHAUSTED link — reachable, the unlock edit
/// never checks claims remaining — would render as a wrapped gift nobody can
/// ever open). The tolerance lives in domain instead: `can_claim_if_unsealed`
/// asks "claimable if it weren't wrapped?", and a future refusal variant is
/// handled at its definition site.
fn link_waits(l: &Link, now: OffsetDateTime) -> bool {
    l.can_claim_if_unsealed(now).is_ok()
}
fn link_is_open_door(l: &Link, now: OffsetDateTime) -> bool {
    l.can_claim(now).is_ok()
}

fn recipient(l: &Link, friends: &HashMap<String, String>) -> String {
    l.friend_id
        .as_ref()
        .and_then(|id| friends.get(id).cloned())
        .unwrap_or_else(|| l.label.clone())
}

pub fn compose_scrapbook(
    claims: Vec<Claim>,
    links: Vec<Link>,
    friends: Vec<Friend>,
    games: HashMap<String, Game>,
    now: OffsetDateTime,
) -> ScrapbookView {
    let friend_names: HashMap<String, String> =
        friends.into_iter().map(|f| (f.id, f.name)).collect();
    let links_by_token: HashMap<String, &Link> =
        links.iter().map(|l| (l.token.clone(), l)).collect();

    // Sort BEFORE formatting: string-sorting mixed-precision RFC3339 missorts
    // ("…08.123Z" < "…08Z" lexically while 08.123 > 08 in time).
    let mut claims = claims;
    claims.sort_by(|a, b| {
        (a.created_at, a.game_id.as_str()).cmp(&(b.created_at, b.game_id.as_str()))
    });

    let mut entries = Vec::new();
    let mut orphan_claim_count = 0u32;
    let mut stale_pending_count = 0u32;

    for c in &claims {
        if c.link_token == SELF_LINK_TOKEN {
            continue; // structural drop, BEFORE the join (spec B3)
        }
        let Some(link) = links_by_token.get(&c.link_token) else {
            // an orphan that is not SELF is a data fact — counted, never skipped
            orphan_claim_count += 1;
            continue;
        };
        match c.state {
            ClaimState::Fulfilled => {}
            ClaimState::Pending => {
                if now - c.created_at > time::Duration::hours(STALE_PENDING_HOURS) {
                    stale_pending_count += 1;
                    continue;
                }
            }
            ClaimState::Compensated | ClaimState::Failed => continue,
        }
        entries.push(ScrapbookEntry {
            claimed_at: rfc3339(c.created_at),
            state: c.state,
            game: scrapbook_game(&c.game_id, &games),
            recipient: recipient(link, &friend_names),
            gift_note: link.gift_note.clone(),
            tag: link
                .curated_notes
                .as_ref()
                .and_then(|m| m.get(&c.game_id).cloned()),
            thank_note: link.thank_note.clone(),
            thanked_at: link.thanked_at.map(rfc3339),
            link_token: link.token.clone(),
            link_label: link.label.clone(),
        });
    }

    let mut waiting = Vec::new();
    let mut doors_open = Vec::new();
    let mut sorted_links: Vec<&Link> = links.iter().collect();
    sorted_links.sort_by(|a, b| (a.created_at, &a.token).cmp(&(b.created_at, &b.token)));
    for link in sorted_links {
        let curated = link.curated_game_ids.as_deref().unwrap_or(&[]);
        if curated.is_empty() {
            if !link_is_open_door(link, now) {
                continue; // revoked, expired, exhausted — or SEALED: wrapped is not open
            }
            doors_open.push(ScrapbookDoor {
                link_token: link.token.clone(),
                link_label: link.label.clone(),
                recipient: recipient(link, &friend_names),
                claims_left: link.claims_allowed.saturating_sub(link.claims_used),
                created_at: rfc3339(link.created_at),
            });
            continue;
        }
        if !link_waits(link, now) {
            continue; // dead links leave waiting; a SEAL alone does not
        }
        // THE LISTABILITY TEST, not a claim-absence test (spec B1/B2): Failed
        // retires, hidden/non-giftable were taken off the shelf, a claimed or
        // stuck-pending game has left `Available` — is_listable covers all of it.
        let games_waiting: Vec<ScrapbookGame> = curated
            .iter()
            .filter(|id| games.get(*id).is_some_and(|g| g.is_listable()))
            .map(|id| scrapbook_game(id, &games))
            .collect();
        if !games_waiting.is_empty() {
            waiting.push(ScrapbookWaitingLink {
                link_token: link.token.clone(),
                link_label: link.label.clone(),
                recipient: recipient(link, &friend_names),
                sealed_until: link.unlock_at.filter(|u| *u > now).map(rfc3339),
                games: games_waiting,
            });
        }
    }

    ScrapbookView {
        entries,
        waiting,
        doors_open,
        orphan_claim_count,
        stale_pending_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{Claim, ClaimState, Friend, Game, GameStatus, Link};
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use time::macros::datetime;

    const NOW: OffsetDateTime = datetime!(2026-09-14 12:00 UTC);

    // SHAPE-only reference: store_test.rs's game() helper shows a full Game
    // literal — different signature, different crate, cannot be imported;
    // the field list is copied, not the arity.
    fn fx_game_with(id: &str, status: GameStatus, giftable: bool, hidden: bool) -> Game {
        Game {
            id: id.into(),
            title: format!("title-{id}"),
            bundle: "B".into(),
            gamekey: "gk".into(),
            machine_name: "mn".into(),
            key_type: "steam".into(),
            giftable,
            hidden,
            status,
            claim_id: None,
            artwork_url: Some(format!("https://art/{id}.png")),
            keyindex: 0,
            requires_choice: false,
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
            hidden_source: None,
            acquired_at: Some(datetime!(2014-08-15 0:00 UTC)),
        }
    }
    fn fx_game(id: &str) -> Game {
        fx_game_with(id, GameStatus::Available, true, false)
    }

    fn fx_link(token: &str) -> Link {
        Link {
            token: token.into(),
            label: format!("label-{token}"),
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 2,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            curated_notes: None,
            friend_id: None,
            created_at: datetime!(2024-09-01 0:00 UTC),
        }
    }

    fn fx_claim(id: &str, token: &str, gid: &str, state: ClaimState, at: OffsetDateTime) -> Claim {
        Claim {
            id: id.into(),
            link_token: token.into(),
            game_id: gid.into(),
            state,
            gift_url: None,
            revealed_key: None,
            created_at: at,
            choice_pre_tpks: None,
            failure_reason: None,
        }
    }

    fn games_map(games: Vec<Game>) -> HashMap<String, Game> {
        games.into_iter().map(|g| (g.id.clone(), g)).collect()
    }

    #[test]
    fn waiting_is_listability_not_claim_absence() {
        // live link curates 5 games in decided states; waiting must contain
        // EXACTLY the listable one (B1/B2 + the stale-Pending arm — that Q2
        // behaviour is DERIVED from this filter, so it is pinned here).
        let mut l = fx_link("t1");
        l.curated_game_ids = Some(vec![
            "g-list".into(),
            "g-exp".into(),
            "g-hid".into(),
            "g-ng".into(),
            "g-pend".into(),
        ]);
        let games = games_map(vec![
            fx_game("g-list"),
            fx_game_with("g-exp", GameStatus::Expired, true, false), // Failed-retired
            fx_game_with("g-hid", GameStatus::Available, true, true),
            fx_game_with("g-ng", GameStatus::Available, false, false),
            fx_game_with("g-pend", GameStatus::Pending, true, false), // stuck mid-claim
        ]);
        let view = compose_scrapbook(vec![], vec![l], vec![], games, NOW);
        assert_eq!(view.waiting.len(), 1);
        assert_eq!(view.waiting[0].games.len(), 1);
        assert_eq!(view.waiting[0].games[0].id, "g-list");
    }

    #[test]
    fn self_claims_dropped_before_join_and_not_orphans() {
        let l = fx_link("t1");
        let claims = vec![
            fx_claim(
                "c-self",
                domain::SELF_LINK_TOKEN,
                "g1",
                ClaimState::Fulfilled,
                datetime!(2026-09-01 12:00 UTC),
            ),
            fx_claim(
                "c1",
                "t1",
                "g1",
                ClaimState::Fulfilled,
                datetime!(2026-09-01 12:00 UTC),
            ),
        ];
        let view = compose_scrapbook(claims, vec![l], vec![], HashMap::new(), NOW);
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].link_token, "t1");
        assert_eq!(view.orphan_claim_count, 0);
    }

    #[test]
    fn non_self_orphan_claim_is_counted_never_skipped() {
        let claims = vec![fx_claim(
            "c1",
            "ghost",
            "g1",
            ClaimState::Fulfilled,
            datetime!(2026-09-01 12:00 UTC),
        )];
        let view = compose_scrapbook(claims, vec![], vec![], HashMap::new(), NOW);
        assert!(view.entries.is_empty());
        assert_eq!(view.orphan_claim_count, 1);
    }

    #[test]
    fn revoked_link_keeps_entry_loses_waiting() {
        // ONE fixture, two assertions (Q4): the gift happened; revocation is
        // about the future.
        let mut l = fx_link("t1");
        l.revoked = true;
        l.curated_game_ids = Some(vec!["g-w".into()]);
        let claims = vec![fx_claim(
            "c1",
            "t1",
            "g-done",
            ClaimState::Fulfilled,
            datetime!(2023-09-20 12:00 UTC),
        )];
        let games = games_map(vec![fx_game("g-w"), fx_game("g-done")]);
        let view = compose_scrapbook(claims, vec![l], vec![], games, NOW);
        assert_eq!(view.entries.len(), 1, "the claim IS in entries");
        assert!(view.waiting.is_empty(), "the unclaimed game is NOT waiting");
    }

    #[test]
    fn pending_boundary_47h_badges_49h_drops_and_counts() {
        // fixtures ON the boundary (frozen clock — a fixture aged 3h asserts
        // nothing about a 48h rule).
        let l = fx_link("t1");
        let claims = vec![
            fx_claim(
                "c-47",
                "t1",
                "g1",
                ClaimState::Pending,
                NOW - time::Duration::hours(47),
            ),
            fx_claim(
                "c-49",
                "t1",
                "g2",
                ClaimState::Pending,
                NOW - time::Duration::hours(49),
            ),
        ];
        let view = compose_scrapbook(claims, vec![l], vec![], HashMap::new(), NOW);
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].game.id, "g1");
        assert_eq!(view.entries[0].state, ClaimState::Pending);
        assert_eq!(view.stale_pending_count, 1);
    }

    #[test]
    fn recipient_prefers_friend_name_falls_back_to_label() {
        let mut la = fx_link("ta");
        la.friend_id = Some("f1".into());
        let lb = fx_link("tb"); // label-tb, no friend
        let friends = vec![Friend {
            id: "f1".into(),
            name: "sam".into(),
            shelf_token: "ab".repeat(32),
            created_at: datetime!(2026-09-04 12:00 UTC),
        }];
        let claims = vec![
            fx_claim(
                "c-a",
                "ta",
                "g1",
                ClaimState::Fulfilled,
                datetime!(2026-09-01 12:00 UTC),
            ),
            fx_claim(
                "c-b",
                "tb",
                "g2",
                ClaimState::Fulfilled,
                datetime!(2026-09-02 12:00 UTC),
            ),
        ];
        let view = compose_scrapbook(claims, vec![la, lb], friends, HashMap::new(), NOW);
        let recipients: Vec<&str> = view.entries.iter().map(|e| e.recipient.as_str()).collect();
        assert_eq!(recipients, vec!["sam", "label-tb"]);
    }

    #[test]
    fn entries_sorted_claimed_at_then_game_id() {
        let l = fx_link("t1");
        let t_early = datetime!(2026-09-01 12:00 UTC);
        let t_tie = datetime!(2026-09-02 12:00 UTC);
        let claims = vec![
            fx_claim("c-tie-b", "t1", "g-b", ClaimState::Fulfilled, t_tie),
            fx_claim("c-early", "t1", "g-z", ClaimState::Fulfilled, t_early),
            fx_claim("c-tie-a", "t1", "g-a", ClaimState::Fulfilled, t_tie),
        ];
        let view = compose_scrapbook(claims, vec![l], vec![], HashMap::new(), NOW);
        let ids: Vec<&str> = view.entries.iter().map(|e| e.game.id.as_str()).collect();
        assert_eq!(ids, vec!["g-z", "g-a", "g-b"], "(claimed_at, game_id) asc");
    }

    #[test]
    fn doors_are_uncurated_live_links_with_created_at() {
        let open = fx_link("t-open"); // uncurated, live
        let mut revoked = fx_link("t-rev");
        revoked.revoked = true; // uncurated but dead
        let mut curated = fx_link("t-cur");
        curated.curated_game_ids = Some(vec!["g1".into()]);
        let games = games_map(vec![fx_game("g1")]);
        let view = compose_scrapbook(vec![], vec![open, revoked, curated], vec![], games, NOW);
        assert_eq!(view.doors_open.len(), 1);
        let d = &view.doors_open[0];
        assert_eq!(d.link_token, "t-open");
        assert_eq!(d.claims_left, 2);
        assert_eq!(d.created_at, "2024-09-01T00:00:00Z");
        // and the curated link is in waiting, not doors
        assert_eq!(view.waiting.len(), 1);
        assert_eq!(view.waiting[0].link_token, "t-cur");
    }

    #[test]
    fn sealed_two_half_waits_labeled_never_doors() {
        // sealed curated → IS in waiting, labeled; sealed uncurated → NOT in
        // doors (nowhere in v1). Controls: unsealed → sealed_until None; a
        // PAST unlock also None (sealed-AT-now, not attribute-present).
        let mut sealed_cur = fx_link("t-sc");
        sealed_cur.unlock_at = Some(datetime!(2026-09-24 12:00 UTC)); // NOW + 10d
        sealed_cur.curated_game_ids = Some(vec!["g1".into()]);
        let mut sealed_uncur = fx_link("t-su");
        sealed_uncur.unlock_at = Some(datetime!(2026-09-24 12:00 UTC));
        let mut past_unlock = fx_link("t-past");
        past_unlock.unlock_at = Some(NOW - time::Duration::days(1));
        past_unlock.curated_game_ids = Some(vec!["g2".into()]);
        // both masking arms — states the admin write paths forbid or never
        // check, constructed freely here (the argument for the pure fn):
        let mut sealed_expired = fx_link("t-se");
        sealed_expired.unlock_at = Some(datetime!(2026-09-24 12:00 UTC));
        sealed_expired.expires_at = Some(NOW - time::Duration::days(1));
        sealed_expired.curated_game_ids = Some(vec!["g3".into()]);
        let mut sealed_exhausted = fx_link("t-sx");
        sealed_exhausted.unlock_at = Some(datetime!(2026-09-24 12:00 UTC));
        sealed_exhausted.claims_used = sealed_exhausted.claims_allowed;
        sealed_exhausted.curated_game_ids = Some(vec!["g4".into()]);
        let games = games_map(vec![
            fx_game("g1"),
            fx_game("g2"),
            fx_game("g3"),
            fx_game("g4"),
        ]);
        let view = compose_scrapbook(
            vec![],
            vec![
                sealed_cur,
                sealed_uncur,
                past_unlock,
                sealed_expired,
                sealed_exhausted,
            ],
            vec![],
            games,
            NOW,
        );
        assert!(view.doors_open.is_empty(), "a wrapped door is not open");
        let tokens: Vec<&str> = view.waiting.iter().map(|w| w.link_token.as_str()).collect();
        // order is the deterministic (created_at, token) sort — same created_at
        // fixture-wide, so token decides: "t-past" < "t-sc"
        assert_eq!(
            tokens,
            vec!["t-past", "t-sc"],
            "sealed∧expired and sealed∧exhausted see through the mask; sealed-curated and past-unlock stay"
        );
        assert_eq!(view.waiting[0].sealed_until, None, "past unlock is OPEN");
        assert_eq!(
            view.waiting[1].sealed_until.as_deref(),
            Some("2026-09-24T12:00:00Z")
        );
    }

    #[test]
    fn tag_read_only_for_entry_games() {
        let mut l = fx_link("t1");
        l.curated_game_ids = Some(vec!["g1".into()]);
        let mut notes = std::collections::BTreeMap::new();
        notes.insert("g1".to_string(), "for the rainy days ♡".to_string());
        notes.insert("g-gone".to_string(), "orphaned note".to_string());
        l.curated_notes = Some(notes);
        let claims = vec![fx_claim(
            "c1",
            "t1",
            "g1",
            ClaimState::Fulfilled,
            datetime!(2026-09-01 12:00 UTC),
        )];
        let view = compose_scrapbook(claims, vec![l], vec![], HashMap::new(), NOW);
        assert_eq!(view.entries[0].tag.as_deref(), Some("for the rainy days ♡"));
        // the orphaned note (curated_notes outliving curated_game_ids) changes
        // nothing anywhere — the games map is empty, so g1 fails the listability
        // lookup and no waiting row exists; the orphan note never surfaces.
        assert!(view.waiting.is_empty());
    }
}
