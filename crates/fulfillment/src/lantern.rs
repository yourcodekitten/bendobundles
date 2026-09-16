//! The lantern 🏮 — spec: docs/spec-lantern.md. PURE: buckets, composition, rendering. The handler
//! in lib.rs owns reads + the record→send→mark writes. Nothing here may name WHISPER#.
//!
//! Load-bearing rules, each pinned below:
//! - Slot key = Sunday DATE; boundary = Sunday 21:00Z; bucket = [end−7d, end). The Sunday tick
//!   (21:05Z EDT / 22:05Z EST) always closes its own bucket, so nothing is skipped or doubled
//!   across DST (OMBB B2b) — under EST the 21:00–22:05Z span rolls forward a week, never lost.
//! - Doors/wrapped mention on BUCKET(k) (past birthdays); closing on BUCKET(k+1) (Lilith).
//! - A Wednesday `now` maps to the previous Sunday (B1): the heartbeat can never win a slot.
//! - Every foreign field goes through `bell::field` (sanitise → cap → escape) EXACTLY once.
//! - Liveness (`is_open_door`, chimney age) is evaluated at `now`, buckets at the slot:
//!   "same bucket, current liveness" (OMBB). An expiry in the boundary→tick span (21:00–21:05Z
//!   under EDT, 21:00–22:05Z under EST) is in BUCKET(k+1) but already past at the tick, so
//!   liveness drops it — harmless, the door is closed; "never dropped" is a doors/wrapped
//!   property, not closing's.

use crate::bell::{cap_content, field};
use domain::{Claim, ClaimState, Game, Link};
use std::collections::HashMap;
use time::{Date, Duration, Month, OffsetDateTime, Time, UtcOffset, Weekday};

pub const ROOM_CAP: usize = 5;
const LABEL_MAX: usize = 120;
const TITLE_MAX: usize = 240;
const BOUNDARY: Time = time::macros::time!(21:00);
/// The sweep's own bar (`pending_age_sweep`): a Pending older than this is a defect, not a gift
/// mid-open. Mirrors `crate::RECONCILE_STUCK_ALERT_AGE`; re-declared so this module stays
/// I/O-free. Kept equal by `chimney_bar_matches_the_sweep`.
const CHIMNEY_BAR: Duration = Duration::hours(24);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub sunday: Date,
}

impl Slot {
    /// `YYYY-MM-DD` — `Date`'s Display; the store key suffix.
    pub fn key(&self) -> String {
        self.sunday.to_string()
    }
    pub fn end(&self) -> OffsetDateTime {
        self.sunday.with_time(BOUNDARY).assume_utc()
    }
    pub fn start(&self) -> OffsetDateTime {
        self.end() - Duration::days(7)
    }
    pub fn next(&self) -> Slot {
        Slot {
            sunday: self.sunday + Duration::days(7),
        }
    }
    pub fn contains(&self, t: OffsetDateTime) -> bool {
        self.start() <= t && t < self.end()
    }
}

/// The slot a tick at `now` belongs to: the latest Sunday 21:00Z boundary at or before `now`.
pub fn tick_slot(now: OffsetDateTime) -> Slot {
    let now = now.to_offset(UtcOffset::UTC);
    let back = i64::from(now.weekday().number_days_from_sunday());
    let mut sunday = now.date() - Duration::days(back);
    if back == 0 && now.time() < BOUNDARY {
        sunday -= Duration::days(7);
    }
    Slot { sunday }
}

/// America/New_York offset for `t` — the US rule (2nd Sunday of March 02:00 local → 1st Sunday
/// of November 02:00 local). Hand-rolled because the `time` crate ships no tz database and this
/// is the only place the lantern needs local time (dates in the message). Pinned across DST.
fn eastern_offset(t: OffsetDateTime) -> UtcOffset {
    let y = t.year();
    let nth_sunday = |m: Month, n: u8| -> Date {
        let first = Date::from_calendar_date(y, m, 1).expect("valid");
        let to_sun = (7 - first.weekday().number_days_from_sunday()) % 7;
        first + Duration::days(i64::from(to_sun) + 7 * i64::from(n - 1))
    };
    // transitions at 02:00 local = 07:00Z (EST→EDT) and 06:00Z (EDT→EST)
    let dst_start = nth_sunday(Month::March, 2)
        .with_time(time::macros::time!(07:00))
        .assume_utc();
    let dst_end = nth_sunday(Month::November, 1)
        .with_time(time::macros::time!(06:00))
        .assume_utc();
    if t >= dst_start && t < dst_end {
        UtcOffset::from_hms(-4, 0, 0).expect("edt")
    } else {
        UtcOffset::from_hms(-5, 0, 0).expect("est")
    }
}

/// "sep 20" — lowercase month + day, in America/New_York (ben reads local).
pub fn eastern_date(t: OffsetDateTime) -> String {
    let l = t.to_offset(eastern_offset(t));
    let m = match l.month() {
        Month::January => "jan",
        Month::February => "feb",
        Month::March => "mar",
        Month::April => "apr",
        Month::May => "may",
        Month::June => "jun",
        Month::July => "jul",
        Month::August => "aug",
        Month::September => "sep",
        Month::October => "oct",
        Month::November => "nov",
        Month::December => "dec",
    };
    format!("{m} {}", l.day())
}

fn weekday_name(t: OffsetDateTime) -> &'static str {
    match t.to_offset(eastern_offset(t)).weekday() {
        Weekday::Monday => "monday",
        Weekday::Tuesday => "tuesday",
        Weekday::Wednesday => "wednesday",
        Weekday::Thursday => "thursday",
        Weekday::Friday => "friday",
        Weekday::Saturday => "saturday",
        Weekday::Sunday => "sunday",
    }
}

pub struct Input<'a> {
    pub links: &'a [Link],
    pub pending: &'a [Claim],
    pub games: &'a HashMap<String, Game>,
    /// friend_id → name (admin-written; still goes through `field`).
    pub friends: &'a HashMap<String, String>,
    pub slot: Slot,
    pub now: OffsetDateTime,
    /// Has ANY lantern ever been delivered? (`quiet` rows do not count.) Gates the backlog line.
    pub any_delivered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub heading: String,
    pub lines: Vec<String>,
    /// Lines beyond ROOM_CAP — rendered as "· and N more".
    pub more: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lantern {
    pub rooms: Vec<Room>,
    /// doors, chimney, wrapped, closing — what compose JUDGED (logged on quiet too).
    pub counts: [u32; 4],
}

fn is_shelf(l: &Link) -> bool {
    l.claims_allowed > 1 && l.friend_id.is_none() && l.curated_game_ids.is_none()
}

fn recipient(l: &Link, friends: &HashMap<String, String>) -> String {
    let raw = l
        .friend_id
        .as_ref()
        .and_then(|id| friends.get(id))
        .map(String::as_str)
        .unwrap_or(&l.label);
    field(raw, LABEL_MAX)
}

fn title(game_id: &str, games: &HashMap<String, Game>) -> String {
    // an orphan id renders AS the id — counted, never skipped (the scrapbook's rule)
    field(
        games
            .get(game_id)
            .map(|g| g.title.as_str())
            .unwrap_or(game_id),
        TITLE_MAX,
    )
}

fn room(heading: &str, mut lines: Vec<String>) -> Option<Room> {
    if lines.is_empty() {
        return None;
    }
    let more = lines.len().saturating_sub(ROOM_CAP) as u32;
    lines.truncate(ROOM_CAP);
    Some(Room {
        heading: heading.into(),
        lines,
        more,
    })
}

pub fn compose(input: &Input) -> Option<Lantern> {
    let Input {
        links,
        pending,
        games,
        friends,
        slot,
        now,
        any_delivered,
    } = input;
    let now = *now;
    let next = slot.next();

    // 🚪 doors — open, zero claims, birthday 14d / 60d inside THIS bucket; oldest first
    let mut open: Vec<&Link> = links
        .iter()
        .filter(|l| l.is_open_door(now) && l.claims_used == 0)
        .collect();
    open.sort_by_key(|l| l.created_at);
    let mut doors = Vec::new();
    let mut backlog = 0u32;
    for l in &open {
        let b14 = l.created_at + Duration::days(14);
        let b60 = l.created_at + Duration::days(60);
        let who = recipient(l, friends);
        let left = l.claims_allowed - l.claims_used;
        if slot.contains(b14) {
            doors.push(if is_shelf(l) {
                format!(
                    "· the shelf for {who} — {} slots, nobody's taken one in two weeks",
                    l.claims_allowed
                )
            } else {
                format!("· the door for {who} — open two weeks, nobody's come through yet")
            });
        } else if slot.contains(b60) {
            doors.push(if is_shelf(l) {
                format!(
                    "· the shelf for {who} — {} slots, nobody's taken one in two months. shall it stay open?",
                    l.claims_allowed
                )
            } else {
                format!(
                    "· the door for {who} — open two months, {left} still inside. shall it stay open?"
                )
            });
        } else if !any_delivered && b60 < slot.start() {
            backlog += 1;
        }
    }
    let door_count = doors.len() as u32;
    if backlog > 0 {
        // FIRST, so the room cap can never push it into "and N more" on the one tick that has it
        doors.insert(
            0,
            format!("· and {backlog} doors older than two months nobody has walked through"),
        );
    }

    // 🕯️ chimney — Pending past the bar, every week, with a week counter and the clearing action
    let mut stuck: Vec<&Claim> = pending
        .iter()
        .filter(|c| c.state == ClaimState::Pending && now - c.created_at >= CHIMNEY_BAR)
        .collect();
    stuck.sort_by_key(|c| c.created_at);
    let chimney: Vec<String> = stuck
        .iter()
        .map(|c| {
            let week = (now - c.created_at).whole_days() / 7;
            format!(
                "· {} — a claim started {} never finished (week {week}). it clears when the claim is compensated (slot returned, game re-listed) or fulfilled — no admin button for that yet, see #234",
                title(&c.game_id, games),
                eastern_date(c.created_at)
            )
        })
        .collect();

    // 🎁 wrapped — unlocked, unopened, unlock+7d inside THIS bucket
    let mut wrapped = Vec::new();
    for l in links
        .iter()
        .filter(|l| l.claims_used == 0 && l.is_open_door(now))
    {
        if let Some(u) = l.unlock_at
            && slot.contains(u + Duration::days(7))
        {
            wrapped.push(format!(
                "· the gift for {} — openable since {}, still wrapped a week later",
                recipient(l, friends),
                eastern_date(u)
            ));
        }
    }

    // ⏳ closing — live with claims left, expiring in the NEXT bucket (looks forward)
    let mut soon: Vec<&Link> = links
        .iter()
        .filter(|l| l.is_open_door(now) && l.expires_at.is_some_and(|e| next.contains(e)))
        .collect();
    soon.sort_by_key(|l| l.expires_at);
    let closing: Vec<String> = soon
        .iter()
        .map(|l| {
            let e = l.expires_at.expect("filtered");
            let left = l.claims_allowed - l.claims_used;
            format!(
                "· the door for {} closes {} with {left} claim{} left",
                recipient(l, friends),
                weekday_name(e),
                if left == 1 { "" } else { "s" }
            )
        })
        .collect();

    let counts = [
        door_count,
        chimney.len() as u32,
        wrapped.len() as u32,
        closing.len() as u32,
    ];
    let rooms: Vec<Room> = [
        room(
            "🚪 doors nobody has walked through  ↗ {site}/admin/links",
            doors,
        ),
        room("🕯️ stuck in the chimney  ↗ {site}/admin/ops", chimney),
        room(
            "🎁 wrapped, and past its day  ↗ {site}/admin/links",
            wrapped,
        ),
        room("⏳ doors closing soon  ↗ {site}/admin/links", closing),
    ]
    .into_iter()
    .flatten()
    .collect();
    if rooms.is_empty() {
        None
    } else {
        Some(Lantern { rooms, counts })
    }
}

/// One `content` message, no embeds, mentions structurally denied. `{site}` in headings is
/// substituted here so compose stays URL-free. ONE deep link per room heading (not per line).
pub fn render(l: &Lantern, slot: &Slot, site_url: &str, preview: bool) -> serde_json::Value {
    let mut content = format!(
        "🏮 the lantern · week of {}{}\n",
        eastern_date(slot.start()),
        if preview {
            " (preview — nothing recorded)"
        } else {
            ""
        }
    );
    for r in &l.rooms {
        content.push('\n');
        content.push_str(&r.heading.replace("{site}", site_url));
        content.push('\n');
        for line in &r.lines {
            content.push_str(line);
            content.push('\n');
        }
        if r.more > 0 {
            content.push_str(&format!("· and {} more\n", r.more));
        }
    }
    serde_json::json!({
        "content": cap_content(content.trim_end()),
        "embeds": [],
        "allowed_mentions": { "parse": [] },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{ClaimState, GameStatus};
    use time::macros::datetime;

    fn link(token: &str, created: OffsetDateTime) -> Link {
        Link {
            token: token.into(),
            label: format!("label-{token}"),
            gift_note: None,
            thank_note: None,
            thanked_at: None,
            claims_allowed: 1,
            claims_used: 0,
            revoked: false,
            expires_at: None,
            unlock_at: None,
            curated_game_ids: None,
            curated_notes: None,
            friend_id: None,
            created_at: created,
        }
    }
    // `Game` and `Claim` do NOT impl Default (measured) — every field is listed.
    fn game(id: &str, title: &str) -> Game {
        Game {
            id: id.into(),
            title: title.into(),
            bundle: "b".into(),
            gamekey: "gk".into(),
            machine_name: id.into(),
            key_type: "steam".into(),
            giftable: true,
            hidden: false,
            status: GameStatus::Available,
            claim_id: None,
            artwork_url: None,
            keyindex: 0,
            requires_choice: false,
            steam_app_id: None,
            appid_source: None,
            owned_by_ben: false,
            hidden_source: None,
            acquired_at: None,
        }
    }
    fn claim(id: &str, gid: &str, at: OffsetDateTime) -> Claim {
        Claim {
            id: id.into(),
            link_token: "SELF".into(),
            game_id: gid.into(),
            state: ClaimState::Pending,
            gift_url: None,
            revealed_key: None,
            created_at: at,
            choice_pre_tpks: None,
            failure_reason: None,
        }
    }
    fn input<'a>(
        links: &'a [Link],
        pending: &'a [Claim],
        games: &'a HashMap<String, Game>,
        friends: &'a HashMap<String, String>,
        now: OffsetDateTime,
        any_delivered: bool,
    ) -> Input<'a> {
        Input {
            links,
            pending,
            games,
            friends,
            slot: tick_slot(now),
            now,
            any_delivered,
        }
    }
    const SUN_TICK: OffsetDateTime = datetime!(2026-09-20 21:05 UTC);

    #[test]
    fn tick_slot_maps_every_instant_to_the_sunday_whose_2100z_boundary_it_follows() {
        assert_eq!(
            tick_slot(datetime!(2026-09-20 21:00 UTC)).key(),
            "2026-09-20"
        );
        assert_eq!(
            tick_slot(datetime!(2026-09-20 21:05 UTC)).key(),
            "2026-09-20",
            "the real EDT tick"
        );
        assert_eq!(
            tick_slot(datetime!(2026-09-20 20:59:59 UTC)).key(),
            "2026-09-13",
            "before the boundary is LAST week — why the tick is 17:05 not 17:00"
        );
        assert_eq!(
            tick_slot(datetime!(2026-09-23 21:05 UTC)).key(),
            "2026-09-20",
            "wednesday → previous sunday"
        );
        assert_eq!(
            tick_slot(datetime!(2026-11-01 22:05 UTC)).key(),
            "2026-11-01",
            "the real EST tick, first Sunday after fall-back"
        );
        assert_eq!(
            tick_slot(datetime!(2026-10-25 21:05 UTC)).next().key(),
            "2026-11-01",
            "EDT tick's next IS the EST tick's slot"
        );
        assert_eq!(
            tick_slot(datetime!(2027-03-14 21:05 UTC)).key(),
            "2027-03-14",
            "first EDT tick after spring-forward"
        );
        let s = tick_slot(SUN_TICK);
        assert_eq!(s.start(), datetime!(2026-09-13 21:00 UTC));
        assert_eq!(s.end(), datetime!(2026-09-20 21:00 UTC));
        assert!(s.contains(datetime!(2026-09-20 20:59:59 UTC)) && !s.contains(s.end()));
        assert_eq!(s.next().key(), "2026-09-27");
    }

    #[test]
    fn weekly_buckets_partition_time_exactly_including_the_dst_change_weeks() {
        // Buckets are fixed 21:00Z boundaries, so nothing here is DST-dependent BY CONSTRUCTION —
        // that is the point (B2b): the 169h/167h tick-to-tick weeks around the fall-back
        // (2026-11-01) and spring-forward (2027-03-14) still map every instant to ONE bucket.
        for (a, b) in [
            (
                datetime!(2026-10-25 21:05 UTC),
                datetime!(2026-11-01 22:05 UTC),
            ),
            (
                datetime!(2027-03-07 22:05 UTC),
                datetime!(2027-03-14 21:05 UTC),
            ),
        ] {
            let slots = [tick_slot(a), tick_slot(b), tick_slot(b).next()];
            assert_eq!(slots[0].next(), slots[1]);
            let mut t = a;
            while t < b + time::Duration::hours(3) {
                let n = slots.iter().filter(|s| s.contains(t)).count();
                assert_eq!(n, 1, "{t} in {n} buckets");
                t += time::Duration::minutes(17);
            }
        }
    }

    #[test]
    fn chimney_bar_matches_the_sweep() {
        assert_eq!(CHIMNEY_BAR, crate::RECONCILE_STUCK_ALERT_AGE);
    }

    #[test]
    fn doors_mention_on_the_14d_and_60d_birthdays_and_never_otherwise() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        // birthday instant = created + 14d must fall inside [09-13 21:00Z, 09-20 21:00Z)
        let inside = link(
            "in",
            slot.start() - time::Duration::days(14) + time::Duration::seconds(1),
        );
        let at_end = link("end", slot.end() - time::Duration::days(14)); // +14d == end ⇒ NOT in
        let before = link(
            "before",
            slot.start() - time::Duration::days(14) - time::Duration::seconds(1),
        );
        let sixty = link(
            "sixty",
            slot.start() - time::Duration::days(60) + time::Duration::hours(1),
        );
        let links = vec![inside, at_end, before, sixty];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let doors = &l.rooms[0];
        assert_eq!(doors.lines.len(), 2, "{:?}", doors.lines);
        // oldest first: `sixty` (created ~60d ago) precedes `in` (~14d ago); `-` is not a
        // Markdown metacharacter, so labels render unescaped
        assert!(
            doors.lines[0].contains("label-sixty") && doors.lines[0].contains("shall it stay open"),
            "{:?}",
            doors.lines
        );
        assert!(
            doors.lines[1].contains("label-in") && doors.lines[1].contains("two weeks"),
            "{:?}",
            doors.lines
        );
        assert_eq!(l.counts, [2, 0, 0, 0]);
    }

    #[test]
    fn shelf_voice_vs_door_voice() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let mut shelf = link(
            "shelf",
            slot.start() - time::Duration::days(14) + time::Duration::hours(1),
        );
        shelf.claims_allowed = 15;
        let mut door = link(
            "door",
            slot.start() - time::Duration::days(14) + time::Duration::hours(2),
        );
        door.claims_allowed = 5;
        door.friend_id = Some("f1".into());
        let links = vec![shelf, door];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let lines = &l.rooms[0].lines;
        assert!(
            lines[0].contains("the shelf for") && lines[0].contains("15 slots"),
            "{lines:?}"
        );
        assert!(lines[1].contains("the door for"), "{lines:?}");
    }

    #[test]
    fn backlog_line_counts_doors_past_sixty_days_only_until_first_delivery() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        let links = vec![
            link("old1", SUN_TICK - time::Duration::days(70)),
            link("old2", SUN_TICK - time::Duration::days(200)),
            link("young", SUN_TICK - time::Duration::days(40)),
        ];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, false)).unwrap();
        assert!(
            l.rooms[0].lines[0].contains("2 doors older than two months"),
            "backlog line is FIRST: {:?}",
            l.rooms[0].lines
        );
        assert!(
            compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).is_none(),
            "delivered once ⇒ backlog gone ⇒ quiet"
        );
    }

    #[test]
    fn chimney_lists_pending_past_24h_with_week_counter_and_names_the_action() {
        let mut games = HashMap::new();
        games.insert("g1".into(), game("g1", "Soulcalibur *VI*"));
        let friends = HashMap::new();
        let links: Vec<Link> = vec![];
        let pending = vec![
            claim("c1", "g1", SUN_TICK - time::Duration::days(72)),
            claim("c2", "g1", SUN_TICK - time::Duration::hours(23)),
        ];
        let l = compose(&input(&links, &pending, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l
            .rooms
            .iter()
            .find(|r| r.heading.contains("chimney"))
            .unwrap();
        assert_eq!(r.lines.len(), 1, "{:?}", r.lines);
        assert!(
            r.lines[0].contains("week 10")
                && r.lines[0].contains("compensated")
                && r.lines[0].contains("#234")
        );
        assert!(
            r.lines[0].contains(r"Soulcalibur \*VI\*") && !r.lines[0].contains(r"\\*"),
            "escaped exactly once: {}",
            r.lines[0]
        );
    }

    #[test]
    fn wrapped_mentions_seven_days_after_unlock_once() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        let mut w = link("w", SUN_TICK - time::Duration::days(100));
        w.unlock_at = Some(slot.start() - time::Duration::days(7) + time::Duration::hours(2));
        let mut early = link("early", SUN_TICK - time::Duration::days(100));
        early.unlock_at = Some(slot.start() - time::Duration::days(7) - time::Duration::hours(2));
        let links = vec![w, early];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l
            .rooms
            .iter()
            .find(|r| r.heading.contains("wrapped"))
            .unwrap();
        assert_eq!(r.lines.len(), 1, "{:?}", r.lines);
        assert!(r.lines[0].contains("still wrapped a week later"));
        assert!(r.lines[0].contains("label-w") && !r.lines[0].contains("label-early"));
    }

    #[test]
    fn closing_looks_forward_into_the_next_bucket_only() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        let slot = tick_slot(SUN_TICK);
        // each via link() so LABELS differ too (a clone keeps `label-lastthu`; the review caught it)
        let old = SUN_TICK - time::Duration::days(100);
        let mut last_thu = link("lastthu", old);
        last_thu.expires_at = Some(datetime!(2026-09-17 12:00 UTC)); // already closed at the tick
        let mut next_thu = link("nextthu", old);
        next_thu.expires_at = Some(datetime!(2026-09-24 12:00 UTC));
        // AFTER the tick (21:05Z), not merely after the boundary (21:00Z): an expiry in the
        // boundary→tick span is in BUCKET(k+1) but already past at the tick, so liveness drops it
        // — the module doc's stated exception, and this fixture's first draft hit it.
        let mut tonight = link("tonight", old);
        tonight.expires_at = Some(SUN_TICK + time::Duration::minutes(10));
        let mut far = link("far", old);
        far.expires_at = Some(slot.next().end());
        let links = vec![last_thu, next_thu, tonight, far];
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        let r = l
            .rooms
            .iter()
            .find(|r| r.heading.contains("closing"))
            .unwrap();
        assert_eq!(r.lines.len(), 2, "{:?}", r.lines);
        assert!(
            r.lines
                .iter()
                .all(|x| !x.contains("lastthu") && !x.contains("far")),
            "{:?}",
            r.lines
        );
        assert!(r.lines[0].contains("label-tonight") && r.lines[0].contains("closes sunday"));
        assert!(r.lines[1].contains("label-nextthu") && r.lines[1].contains("closes thursday"));
    }

    #[test]
    fn empty_input_is_quiet_and_room_cap_announces_more() {
        let games = HashMap::new();
        let friends = HashMap::new();
        let none: Vec<Claim> = vec![];
        assert!(compose(&input(&[], &none, &games, &friends, SUN_TICK, true)).is_none());
        let slot = tick_slot(SUN_TICK);
        let links: Vec<Link> = (0..8)
            .map(|i| {
                link(
                    &format!("d{i}"),
                    slot.start() - time::Duration::days(14) + time::Duration::minutes(i),
                )
            })
            .collect();
        let l = compose(&input(&links, &none, &games, &friends, SUN_TICK, true)).unwrap();
        assert_eq!(l.rooms[0].lines.len(), ROOM_CAP);
        assert_eq!(l.rooms[0].more, 3);
    }

    #[test]
    fn render_carries_one_deep_link_per_room_no_mentions_and_the_final_cap() {
        // five chimney titles of 240 '*' each → 480 escaped chars per line → ~2,600 raw: the
        // 2000 cap is REACHED here (the review found the first draft of this test topped out ~1,550)
        let mut games = HashMap::new();
        for i in 0..5 {
            games.insert(format!("g{i}"), game(&format!("g{i}"), &"*".repeat(240)));
        }
        let friends = HashMap::new();
        let links: Vec<Link> = vec![];
        let pending: Vec<Claim> = (0..5)
            .map(|i| {
                claim(
                    &format!("c{i}"),
                    &format!("g{i}"),
                    SUN_TICK - time::Duration::days(3 + i),
                )
            })
            .collect();
        let slot = tick_slot(SUN_TICK);
        let l = compose(&input(&links, &pending, &games, &friends, SUN_TICK, true)).unwrap();
        let raw: usize = l
            .rooms
            .iter()
            .map(|r| r.lines.iter().map(|x| x.chars().count()).sum::<usize>())
            .sum();
        assert!(
            raw > 2000,
            "fixture must overflow the cap to test it: {raw}"
        );
        let v = render(&l, &slot, "https://s", false);
        let c = v["content"].as_str().unwrap();
        assert!(c.starts_with("🏮 the lantern · week of sep 13"), "{c}");
        assert!(c.contains("https://s/admin/ops"));
        assert!(
            !c.contains("token=") && !c.contains("/l/"),
            "no bearer capability: {c}"
        );
        // parity: runs of `\*` mean char 2000 is either `\` (popped ⇒ 1999) or `*` (stays 2000),
        // and the header length decides which — so assert the RANGE, and pin the pop case below.
        let n = c.chars().count();
        assert!((1999..=2000).contains(&n) && !c.ends_with('\\'), "{n}");
        // the cut-on-backslash case, pinned by construction: content that is exactly 2001 chars
        // with the backslash at position 2000 (cap_content is what render calls last)
        let exact = format!("{}\\*", "y".repeat(1999));
        assert_eq!(crate::bell::cap_content(&exact), "y".repeat(1999));
        assert_eq!(v["allowed_mentions"]["parse"].as_array().unwrap().len(), 0);
        assert!(v["embeds"].as_array().unwrap().is_empty());
        let p = render(&l, &slot, "https://s", true);
        assert!(p["content"].as_str().unwrap().contains("(preview"));
    }

    #[test]
    fn eastern_dates_render_local_across_dst() {
        assert_eq!(eastern_date(datetime!(2026-09-21 02:00 UTC)), "sep 20"); // EDT: 22:00 the day before
        assert_eq!(eastern_date(datetime!(2026-12-01 04:30 UTC)), "nov 30"); // EST: 23:30 the day before
        assert_eq!(eastern_date(datetime!(2026-12-01 05:30 UTC)), "dec 1");
        assert_eq!(weekday_name(datetime!(2026-09-24 12:00 UTC)), "thursday");
    }
}
