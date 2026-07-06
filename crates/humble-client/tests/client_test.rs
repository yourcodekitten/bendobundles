use humble_client::{HumbleClient, SessionCookie};
use wiremock::matchers::{body_string_contains, header, method, path, path_regex, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> serde_json::Value {
    serde_json::from_str(&fixture_str(name)).unwrap()
}

fn fixture_str(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

async fn client(server: &MockServer) -> HumbleClient {
    HumbleClient::new(&server.uri(), SessionCookie::new("sekrit".into())).unwrap()
}

#[tokio::test]
async fn lists_gamekeys() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .and(header("cookie", "_simpleauth_sess=sekrit"))
        .and(header("x-requested-by", "hb_android_app"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("user_order.json")))
        .mount(&server)
        .await;

    let keys = client(&server).await.gamekeys().await.unwrap();
    assert_eq!(keys, vec!["AAAAbbbbCCCC", "DDDDeeeeFFFF"]);
}

#[tokio::test]
async fn parses_order_key_states() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/order/AAAAbbbbCCCC"))
        .and(query_param("all_tpkds", "true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture("order_detail.json")))
        .mount(&server)
        .await;

    let order = client(&server).await.order("AAAAbbbbCCCC").await.unwrap();
    assert_eq!(order.bundle_name, "Humble Indie Bundle 99");
    assert_eq!(order.gamekey, "AAAAbbbbCCCC");
    assert_eq!(order.keys.len(), 3);

    let fresh = &order.keys[0];
    assert!(fresh.giftable && !fresh.redeemed && !fresh.expired);

    let revealed = &order.keys[1];
    assert!(revealed.redeemed && !revealed.giftable);

    let dead = &order.keys[2];
    assert!(dead.expired && !dead.giftable);

    assert_eq!(order.keys[0].keyindex, 0);
    assert_eq!(order.keys[1].keyindex, 1);
    assert_eq!(order.keys[2].keyindex, 2);
    assert_eq!(order.subproducts.len(), 2);
    assert_eq!(order.subproducts[0].human_name, "Stardew Valley");
    assert_eq!(
        order.subproducts[0].icon.as_deref(),
        Some("https://hb.imgix.net/stardew.png")
    );
    assert_eq!(order.subproducts[1].icon, None);
}

#[tokio::test]
async fn dead_cookie_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server).await.gamekeys().await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[tokio::test]
async fn forbidden_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = client(&server).await.gamekeys().await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[tokio::test]
async fn login_redirect_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(ResponseTemplate::new(302).append_header("location", "/login"))
        .mount(&server)
        .await;

    let err = client(&server).await.gamekeys().await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[test]
fn cookie_redacts_in_debug() {
    let c = SessionCookie::new("sekrit".into());
    assert_eq!(format!("{c:?}"), "SessionCookie(REDACTED)");
}

#[tokio::test]
async fn redeems_as_gift() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .and(body_string_contains("keytype=stardew_valley_steam"))
        .and(body_string_contains("key=AAAAbbbbCCCC"))
        .and(body_string_contains("keyindex=3"))
        .and(body_string_contains("gift=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "g1ftt0k3n"
        })))
        .mount(&server)
        .await;

    let gift = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam", 3)
        .await
        .unwrap();
    assert_eq!(gift.0, "https://www.humblebundle.com/gift?key=g1ftt0k3n");
}

#[tokio::test]
async fn already_redeemed_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "This key has already been redeemed."
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "already_revealed_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::AlreadyRedeemed));
}

#[tokio::test]
async fn refused_redeem_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "Gifting is disabled for this product."
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "some_product_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::RedeemRefused(ref msg) if msg == "Gifting is disabled for this product."
    ));
}

#[tokio::test]
async fn malformed_redeem_is_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "giftkey": "x"
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "some_product_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Parse(_)));
}

#[tokio::test]
async fn html_200_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/user/order"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html>login</html>")
                .append_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let err = client(&server).await.gamekeys().await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

// ── Humble Choice: choose_content (the pick-spend that precedes the redeem) ──────────────────────

#[tokio::test]
async fn choose_content_gift_sends_the_right_form() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        // The double-submit CSRF pair (cookie value replayed as the header) must be sent — same
        // check the redeem write is held to; without this a broken csrf dance passes silently.
        .and(DoubleSubmitPairMatches)
        .and(body_string_contains("gamekey=UZz2zYTdsC5HfCYp"))
        .and(body_string_contains("parent_identifier=initial"))
        // `chosen_identifiers[]` url-encodes to `chosen_identifiers%5B%5D`.
        .and(body_string_contains(
            "chosen_identifiers%5B%5D=octopathtravelerii",
        ))
        .and(body_string_contains("is_gift=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "force_refresh": true
        })))
        .mount(&server)
        .await;

    client(&server)
        .await
        .choose_content("UZz2zYTdsC5HfCYp", &["octopathtravelerii"], true)
        .await
        .unwrap();
}

#[tokio::test]
async fn choose_content_self_claim_omits_is_gift() {
    let server = MockServer::start().await;
    // The self-claim form must NOT carry is_gift — a mount that requires its absence proves it.
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .and(body_string_contains(
            "chosen_identifiers%5B%5D=cookservedelicious3",
        ))
        .and(NoIsGift)
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "force_refresh": true
        })))
        .mount(&server)
        .await;

    client(&server)
        .await
        .choose_content("f3rpTVdNuy7EBtvm", &["cookservedelicious3"], false)
        .await
        .unwrap();
}

#[tokio::test]
async fn choose_content_multiple_games_repeat_the_field() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .and(body_string_contains("chosen_identifiers%5B%5D=relicta"))
        .and(body_string_contains("chosen_identifiers%5B%5D=levelhead"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "force_refresh": true
        })))
        .mount(&server)
        .await;

    client(&server)
        .await
        .choose_content("gk123", &["relicta", "levelhead"], true)
        .await
        .unwrap();
}

#[tokio::test]
async fn choose_content_refused_is_typed_and_spends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": false,
            "errormsg": "No choices remaining for this month."
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::ChooseFailed { ref reason }
            if reason == "No choices remaining for this month."
    ));
}

#[tokio::test]
async fn choose_content_login_required_is_a_step_up_gate_not_dead_cookie() {
    let server = MockServer::start().await;
    // A HEALTHY-but-gated session answers with login_required — the choose write is on the same
    // secure-area-gated surface as the redeem. The test client has no step-up creds, so the gate
    // surfaces as SecureAreaStepUpFailed (NOT Unauthorized — a dead-cookie misclassification would
    // trip the session self-heal on a cookie that's actually fine).
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "error_id": "login_required"
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::SecureAreaStepUpFailed { .. }
    ));
}

#[tokio::test]
async fn choose_content_html_200_is_unauthorized() {
    let server = MockServer::start().await;
    // A genuinely dead session returns a 200-with-HTML login page (not the login_required JSON) —
    // decode_body maps the leading `<` to Unauthorized. No pick spent.
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html>login</html>")
                .append_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[tokio::test]
async fn choose_content_rate_limited_is_typed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::RateLimited));
}

#[tokio::test]
async fn choose_content_5xx_is_ambiguous_api_not_choose_failed() {
    let server = MockServer::start().await;
    // A 5xx can follow a COMMITTED choose (pick maybe spent) → it must NOT be ChooseFailed (whose
    // contract is "provably not spent", which a caller would re-choose on). It's Api(s), which the
    // caller parks-and-reconciles like the redeem's ambiguous outcomes.
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(502))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Api(502)));
}

#[tokio::test]
async fn choose_content_auth_layer_rejection_is_choose_failed() {
    let server = MockServer::start().await;
    // A 403 with no secureArea redirect is an auth/CSRF-layer rejection — ChooseFailed (park), not
    // a step-up gate and not a dead cookie. No pick spent.
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::ChooseFailed { .. }
    ));
}

#[tokio::test]
async fn choose_content_403_with_login_required_body_is_a_step_up_gate() {
    let server = MockServer::start().await;
    // The step-up gate can appear at the auth layer as a 401/403 carrying a login_required BODY with
    // no secureArea location (redeem_once reads the body here too). choose_once must catch it as a
    // gate, not a plain rejection — else the auto step-up-and-retry never fires. No creds → the gate
    // surfaces as SecureAreaStepUpFailed (NOT ChooseFailed).
    Mock::given(method("POST"))
        .and(path("/humbler/choosecontent"))
        .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
            "error_id": "login_required"
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choose_content("gk123", &["somegame"], true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::SecureAreaStepUpFailed { .. }
    ));
}

#[tokio::test]
async fn choose_content_empty_selection_is_guarded_before_any_request() {
    let server = MockServer::start().await;
    // No mount — if choose_content fired a POST on an empty set, the client would get a wiremock
    // 404 and this would still error, so the mount-free server also proves nothing was sent.
    let err = client(&server)
        .await
        .choose_content("gk123", &[], true)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        humble_client::HumbleError::ChooseFailed { ref reason } if reason.contains("empty")
    ));
    assert_eq!(server.received_requests().await.unwrap().len(), 0);
}

/// Matches only when the request body does NOT contain `is_gift` — for asserting the self-claim
/// form omits it (wiremock has no built-in negative body matcher).
struct NoIsGift;

impl wiremock::Match for NoIsGift {
    fn matches(&self, request: &wiremock::Request) -> bool {
        !std::str::from_utf8(&request.body)
            .map(|b| b.contains("is_gift"))
            .unwrap_or(false)
    }
}

// ── Humble Choice: choice_month (read a month's offered games + state from the membership blob) ──

#[tokio::test]
async fn choice_month_parses_offered_games_and_state() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/membership/may-2021"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(fixture_str("membership_may_2021.html"))
                .append_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let m = client(&server)
        .await
        .choice_month("may-2021")
        .await
        .unwrap();
    assert_eq!(m.gamekey, "May21Gamekey00");
    assert_eq!(m.title, "May 2021");
    assert_eq!(m.product_url_path, "may-2021");
    assert_eq!(m.product_machine_name, "may_2021_choice");
    assert!(m.uses_choices);
    assert!(!m.is_active_content);
    assert!(m.can_redeem_games);
    assert_eq!(m.total_choices, 12);
    // Sorted by machine_name for stable order (the source is a JSON object / HashMap).
    let games: Vec<(&str, &str)> = m
        .offered_games
        .iter()
        .map(|g| (g.machine_name.as_str(), g.title.as_str()))
        .collect();
    assert_eq!(
        games,
        vec![
            ("darksidersgenesis", "Darksiders Genesis"),
            ("metroexodus", "Metro Exodus"),
            ("relicta", "Relicta"),
        ]
    );
}

#[tokio::test]
async fn choice_month_dead_session_is_unauthorized() {
    let server = MockServer::start().await;
    // A dead session serves the plain login page here — no webpack-monthly-product-data blob.
    Mock::given(method("GET"))
        .and(path("/membership/may-2021"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<!DOCTYPE html><html><body>please log in</body></html>")
                .append_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choice_month("may-2021")
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[tokio::test]
async fn choice_month_403_is_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/membership/may-2021"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choice_month("may-2021")
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

#[tokio::test]
async fn choice_month_malformed_blob_is_parse_error() {
    let server = MockServer::start().await;
    // The blob IS present (id matches) but its JSON is truncated/garbage — a distinct path from the
    // dead-session no-blob case, and exactly what a `</script>`-truncation or early tag-cut produces.
    Mock::given(method("GET"))
        .and(path("/membership/may-2021"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "<script id=\"webpack-monthly-product-data\">{ not valid json </script>",
                )
                .append_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .choice_month("may-2021")
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Parse(_)));
}

// ── Humble Choice: choice_months (paginated month enumeration via the cursor path segment) ───────

#[tokio::test]
async fn choice_months_walks_the_cursor_pagination() {
    let server = MockServer::start().await;
    const BASE: &str = "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";
    // Page 1 (bare path) → 2 months + a cursor "CURSOR2"; page 2 (path + cursor) → 1 month, no cursor.
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "cursor": "CURSOR2",
            "products": [
                {
                    "gamekey": "gkMar26", "title": "March 2026", "productUrlPath": "march-2026",
                    "productMachineName": "march_2026_choice", "usesChoices": false,
                    "isActiveContent": true, "canRedeemGames": true,
                    "contentChoiceData": { "game_data": { "gamea": { "title": "Game A" } } }
                },
                {
                    "gamekey": "gkFeb26", "title": "February 2026", "productUrlPath": "february-2026",
                    "productMachineName": "february_2026_choice", "usesChoices": true,
                    "isActiveContent": false, "canRedeemGames": true,
                    "contentChoiceData": { "game_data": {
                        "gamec": { "title": "Game C" }, "gameb": { "title": "Game B" }
                    } }
                }
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/CURSOR2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [
                {
                    "gamekey": "gkJan26", "title": "January 2026", "productUrlPath": "january-2026",
                    "productMachineName": "january_2026_choice", "usesChoices": true,
                    "isActiveContent": false, "canRedeemGames": true,
                    "contentChoiceData": { "game_data": { "gamed": { "title": "Game D" } } }
                }
            ]
        })))
        .mount(&server)
        .await;

    let months = client(&server).await.choice_months(10).await.unwrap();
    assert_eq!(months.len(), 3);
    assert_eq!(months[0].product_machine_name, "march_2026_choice");
    assert!(months[0].is_active_content && !months[0].uses_choices);
    // February's offered games sorted by machine_name.
    let feb = &months[1];
    assert_eq!(feb.product_machine_name, "february_2026_choice");
    let names: Vec<&str> = feb
        .offered_games
        .iter()
        .map(|g| g.machine_name.as_str())
        .collect();
    assert_eq!(names, vec!["gameb", "gamec"]);
    assert_eq!(months[2].product_machine_name, "january_2026_choice");
}

#[tokio::test]
async fn choice_months_single_page_no_cursor_stops() {
    let server = MockServer::start().await;
    const BASE: &str = "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "products": [{
                "gamekey": "gkA", "title": "May 2021", "productUrlPath": "may-2021",
                "productMachineName": "may_2021_choice", "usesChoices": true,
                "isActiveContent": false, "canRedeemGames": true,
                "contentChoiceData": { "game_data": {} }
            }]
        })))
        .mount(&server)
        .await;

    let months = client(&server).await.choice_months(10).await.unwrap();
    assert_eq!(months.len(), 1);
    assert_eq!(months[0].product_machine_name, "may_2021_choice");
}

#[tokio::test]
async fn choice_months_max_pages_bounds_a_nonstop_cursor() {
    let server = MockServer::start().await;
    const BASE: &str = "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";
    // Every page hands back a cursor and a product — the max_pages bound must stop the walk.
    Mock::given(method("GET"))
        .and(path_regex(format!("^{BASE}/")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "cursor": "SAME",
            "products": [{
                "gamekey": "gkX", "title": "X", "productUrlPath": "x",
                "productMachineName": "x_choice", "usesChoices": false,
                "isActiveContent": false, "canRedeemGames": true,
                "contentChoiceData": { "game_data": {} }
            }]
        })))
        .mount(&server)
        .await;

    let months = client(&server).await.choice_months(3).await.unwrap();
    assert_eq!(months.len(), 3); // exactly max_pages products, not an infinite spin
}

#[tokio::test]
async fn choice_months_dead_session_is_unauthorized() {
    let server = MockServer::start().await;
    const BASE: &str = "/api/v1/subscriptions/humble_monthly/subscription_products_with_gamekeys";
    Mock::given(method("GET"))
        .and(path(format!("{BASE}/")))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server).await.choice_months(10).await.unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::Unauthorized));
}

/// Matches when the `csrf-prevention-token` header value equals the `csrf_cookie` value inside
/// the `cookie` header (the double-submit invariant), regardless of what the value is.
struct DoubleSubmitPairMatches;

impl wiremock::Match for DoubleSubmitPairMatches {
    fn matches(&self, request: &wiremock::Request) -> bool {
        let header_val = request
            .headers
            .get("csrf-prevention-token")
            .and_then(|v| v.to_str().ok());
        let cookie_val = request
            .headers
            .get("cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|c| {
                c.split(';')
                    .map(str::trim)
                    .find_map(|kv| kv.strip_prefix("csrf_cookie="))
            });
        match (header_val, cookie_val) {
            (Some(h), Some(c)) => !h.is_empty() && h == c,
            _ => false,
        }
    }
}

#[tokio::test]
async fn redeem_sends_double_submit_csrf_captured_from_preflight() {
    let server = MockServer::start().await;
    // Preflight page GET: humble sets the csrf_cookie alongside the session it was given.
    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("cookie", "_simpleauth_sess=sekrit"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "csrf_cookie=srv-t0k3n; Path=/; Secure"),
        )
        .mount(&server)
        .await;
    // The redeem POST must replay humble's own token as BOTH cookie and header.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .and(header("csrf-prevention-token", "srv-t0k3n"))
        .and(header(
            "cookie",
            "_simpleauth_sess=sekrit; csrf_cookie=srv-t0k3n",
        ))
        .and(body_string_contains("gift=true"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "g1ftt0k3n"
        })))
        .mount(&server)
        .await;

    let gift = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam", 3)
        .await
        .unwrap();
    assert_eq!(gift.0, "https://www.humblebundle.com/gift?key=g1ftt0k3n");
}

#[tokio::test]
async fn redeem_mints_double_submit_pair_when_preflight_sets_no_cookie() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    // No csrf_cookie offered — the client must mint one and keep header == cookie anyway.
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .and(DoubleSubmitPairMatches)
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "giftkey": "g1ftt0k3n"
        })))
        .mount(&server)
        .await;

    let gift = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam", 0)
        .await
        .unwrap();
    assert_eq!(gift.0, "https://www.humblebundle.com/gift?key=g1ftt0k3n");
}

#[tokio::test]
async fn redeem_auth_rejection_is_typed_not_cookie_death() {
    // A 403 on the redeem WRITE is an auth/CSRF-layer rejection, NOT proof the session cookie is
    // dead (reads may still work fine). It must be its own variant so fulfillment doesn't fire
    // the dead-cookie alarm. Live signature captured 2026-07-04: redeem POST 403 while sync
    // walked the full library on the same cookie. The preflight here offers no csrf_cookie, so
    // the error must also report csrf_minted=true — a capture failure is its own signal.
    for status in [401u16, 403, 302] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/humbler/redeemkey"))
            .respond_with(ResponseTemplate::new(status))
            .mount(&server)
            .await;
        let err = client(&server)
            .await
            .redeem_as_gift("AAAAbbbbCCCC", "some_product_steam", 0)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                humble_client::HumbleError::RedeemAuthRejected {
                    status: s,
                    csrf_minted: true
                } if s == status
            ),
            "status {status} must map to RedeemAuthRejected with csrf_minted=true, got {err:?}"
        );
    }
}

#[tokio::test]
async fn captured_token_rejection_reports_csrf_not_minted() {
    // When the preflight DID capture humble's own csrf_cookie and the write still bounces,
    // csrf_minted must be false — that distinguishes "humble rejected its own token" (dance is
    // wrong some other way) from "we never got a token to replay" (capture is broken).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "csrf_cookie=srv-t0k3n; Path=/; Secure"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "some_product_steam", 0)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            humble_client::HumbleError::RedeemAuthRejected {
                status: 403,
                csrf_minted: false
            }
        ),
        "captured-token rejection must report csrf_minted=false, got {err:?}"
    );
}

#[tokio::test]
async fn rejection_with_html_challenge_body_still_types_cleanly() {
    // The rejection arm reads allowlisted headers and DRAINS the 403 body to classify it
    // (`login_required` step-up vs a real rejection) — the body content itself is never logged;
    // the PR#14 body_preview diagnostic was retired once the Cloudflare diagnosis was confirmed.
    // This proves the classification work never alters the contract: a 403 carrying a full HTML
    // body + a cf-mitigated header still returns the same typed RedeemAuthRejected the
    // empty-body path does.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("set-cookie", "csrf_cookie=srv-t0k3n; Path=/; Secure"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(
            ResponseTemplate::new(403)
                .append_header("content-type", "text/html; charset=UTF-8")
                .append_header("cf-mitigated", "challenge")
                .set_body_string(
                    "<!DOCTYPE html>\n<html><head><title>Just a moment...</title></head>\n\
                     <body>Attention Required! | Cloudflare</body></html>",
                ),
        )
        .mount(&server)
        .await;
    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "some_product_steam", 0)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            humble_client::HumbleError::RedeemAuthRejected {
                status: 403,
                csrf_minted: false
            }
        ),
        "an HTML-challenge 403 must still type as RedeemAuthRejected, got {err:?}"
    );
}

#[tokio::test]
async fn ambiguous_redeem_is_typed() {
    // success=true but NO giftkey: humble claims it worked yet handed back nothing. The key may
    // have burned server-side — this must be its own typed outcome, never AlreadyRedeemed.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/humbler/redeemkey"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true
        })))
        .mount(&server)
        .await;

    let err = client(&server)
        .await
        .redeem_as_gift("AAAAbbbbCCCC", "stardew_valley_steam", 0)
        .await
        .unwrap_err();
    assert!(matches!(err, humble_client::HumbleError::AmbiguousRedeem));
}
