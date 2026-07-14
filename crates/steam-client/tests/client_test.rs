fn test_client(server: &wiremock::MockServer) -> steam_client::SteamClient {
    steam_client::SteamClient::new(
        &server.uri(),
        &server.uri(),
        &server.uri(),
        steam_client::SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap()
}

fn test_openid_client(server: &wiremock::MockServer) -> steam_client::SteamClient {
    steam_client::SteamClient::new(
        &server.uri(),
        &server.uri(),
        &server.uri(),
        steam_client::SteamApiKey::new("TESTKEY".into()),
    )
    .unwrap()
}

#[tokio::test]
async fn owned_games_public_returns_appids() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IPlayerService/GetOwnedGames/v0001/"))
        .and(wiremock::matchers::query_param("key", "TESTKEY"))
        .and(wiremock::matchers::query_param("steamid", "76561198000000001"))
        .and(wiremock::matchers::query_param("include_played_free_games", "1"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"game_count":2,"games":[{"appid":413150,"playtime_forever":100},{"appid":1273400,"playtime_forever":0}]}}"#,
        ))
        .mount(&server).await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("76561198000000001".into()))
        .await
        .unwrap();
    assert_eq!(out, steam_client::OwnedGames::Games(vec![413150, 1273400]));
}

#[tokio::test]
async fn owned_games_private_is_absent_game_count() {
    // M4 pin: privacy = response WITHOUT game_count. NOT an error, NOT empty.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/IPlayerService/GetOwnedGames/v0001/",
        ))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(r#"{"response":{}}"#))
        .mount(&server)
        .await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("7656".into()))
        .await
        .unwrap();
    assert_eq!(out, steam_client::OwnedGames::Private);
}

#[tokio::test]
async fn owned_games_zero_count_is_genuinely_empty() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/IPlayerService/GetOwnedGames/v0001/",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string(r#"{"response":{"game_count":0,"games":[]}}"#),
        )
        .mount(&server)
        .await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("7656".into()))
        .await
        .unwrap();
    assert_eq!(out, steam_client::OwnedGames::Games(vec![]));
}

#[tokio::test]
async fn key_rejection_and_rate_limit_are_typed() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path(
            "/IPlayerService/GetOwnedGames/v0001/",
        ))
        .respond_with(wiremock::ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let out = test_client(&server)
        .get_owned_games(&steam_client::SteamId64("x".into()))
        .await;
    assert!(matches!(out, Err(steam_client::SteamError::KeyRejected)));
}

#[tokio::test]
async fn api_key_debug_is_redacted() {
    let k = steam_client::SteamApiKey::new("SECRET123".into());
    assert!(!format!("{k:?}").contains("SECRET123"));
}

#[tokio::test]
async fn persona_and_vanity_parse() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::path("/ISteamUser/GetPlayerSummaries/v0002/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"players":[{"steamid":"7656","personaname":"bendoerr","avatarfull":"https://a/b.jpg"}]}}"#,
        )).mount(&server).await;
    wiremock::Mock::given(wiremock::matchers::path(
        "/ISteamUser/ResolveVanityURL/v0001/",
    ))
    .respond_with(
        wiremock::ResponseTemplate::new(200)
            .set_body_string(r#"{"response":{"success":1,"steamid":"76561198000000001"}}"#),
    )
    .mount(&server)
    .await;
    let c = test_client(&server);
    let p = c
        .get_player_summary(&steam_client::SteamId64("7656".into()))
        .await
        .unwrap();
    assert_eq!(p.name, "bendoerr");
    let id = c.resolve_vanity("bendoerr").await.unwrap();
    assert_eq!(id, steam_client::SteamId64("76561198000000001".into()));
}

// ── OpenID assertion tests ────────────────────────────────────────────────────

fn assertion_params(claimed: &str, return_to: &str) -> Vec<(String, String)> {
    vec![
        (
            "openid.ns".into(),
            "http://specs.openid.net/auth/2.0".into(),
        ),
        ("openid.mode".into(), "id_res".into()),
        ("openid.claimed_id".into(), claimed.into()),
        ("openid.identity".into(), claimed.into()),
        ("openid.return_to".into(), return_to.into()),
        (
            "openid.response_nonce".into(),
            "2026-07-06T00:00:00Znonce".into(),
        ),
        ("openid.assoc_handle".into(), "h".into()),
        // Realistic signed set (field names omit the openid. prefix per OpenID 2.0);
        // MUST include claimed_id or verification rejects it before any network call.
        (
            "openid.signed".into(),
            "signed,op_endpoint,claimed_id,identity,return_to,response_nonce,assoc_handle".into(),
        ),
        ("openid.sig".into(), "sig".into()),
    ]
}

#[tokio::test]
async fn openid_valid_assertion_returns_steamid() {
    let server = wiremock::MockServer::start().await;
    // check_authentication: Steam echoes is_valid:true in key-value form.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains(
            "openid.mode=check_authentication",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:true\n"),
        )
        .mount(&server)
        .await;
    let c = test_openid_client(&server);
    let params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return?ctx=%2Fl%2Fabc",
    );
    let id = c
        .verify_openid_assertion(
            &params,
            "https://bendobundles.com/api/steam/return?ctx=%2Fl%2Fabc",
        )
        .await
        .unwrap();
    assert_eq!(id, steam_client::SteamId64("76561198000000001".into()));
}

#[tokio::test]
async fn openid_invalid_is_rejected() {
    // is_valid:false → OpenIdRejected.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .and(wiremock::matchers::body_string_contains(
            "openid.mode=check_authentication",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("ns:http://specs.openid.net/auth/2.0\nis_valid:false\n"),
        )
        .mount(&server)
        .await;
    let c = test_openid_client(&server);
    let params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return",
    );
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(matches!(
        out,
        Err(steam_client::SteamError::OpenIdRejected(_))
    ));
}

#[tokio::test]
async fn openid_wrong_claimed_id_shape_rejected_without_network() {
    // claimed_id "https://evil.example/openid/id/123" → OpenIdRejected BEFORE any HTTP call.
    // Mount NOTHING; a network attempt would error differently (connection refused / no mock match).
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    let params = assertion_params(
        "https://evil.example/openid/id/123",
        "https://bendobundles.com/api/steam/return",
    );
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(matches!(
        out,
        Err(steam_client::SteamError::OpenIdRejected(_))
    ));
}

#[tokio::test]
async fn openid_return_to_mismatch_rejected() {
    // params say return_to=https://evil.example/... but expected is bendobundles → OpenIdRejected.
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    let params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://evil.example/hijack",
    );
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(matches!(
        out,
        Err(steam_client::SteamError::OpenIdRejected(_))
    ));
}

// ── F1: duplicate security-relevant openid.* keys must be rejected without network ─────────

#[tokio::test]
async fn openid_duplicate_claimed_id_rejected_without_network() {
    // Attack: attacker completes a genuine Steam login for their own id Y, then injects a
    // second openid.claimed_id = X (victim's id) BEFORE the real one.  Our get() takes
    // the first occurrence → returns X; Steam validates Y's signature → is_valid:true.
    // Without a dup guard this would be Ok(SteamId64(X)) — identity forgery.
    //
    // No wiremock mock is mounted. Any network attempt yields SteamError::Api(404) (not
    // OpenIdRejected), so passing this test proves the dup check fires BEFORE HTTP.
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    let mut params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return",
    );
    // Prepend attacker-chosen victim id as a second claimed_id.
    params.insert(
        0,
        (
            "openid.claimed_id".into(),
            "https://steamcommunity.com/openid/id/76561198999999999".into(),
        ),
    );
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(
        matches!(out, Err(steam_client::SteamError::OpenIdRejected(_))),
        "duplicate claimed_id must be rejected before any network call; got: {out:?}"
    );
}

#[tokio::test]
async fn openid_duplicate_return_to_rejected_without_network() {
    // A second openid.return_to could confuse which value Steam validated vs which we checked.
    // No mock mounted — proves pre-network rejection.
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    let mut params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return",
    );
    params.push((
        "openid.return_to".into(),
        "https://evil.example/hijack".into(),
    ));
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(
        matches!(out, Err(steam_client::SteamError::OpenIdRejected(_))),
        "duplicate return_to must be rejected before any network call; got: {out:?}"
    );
}

// ── F8: network errors must not leak the API key embedded in the request URL ────────────────

#[tokio::test]
async fn network_error_does_not_leak_api_key() {
    // Port 1 (tcpmux) is refused on any sane Linux box — immediate ECONNREFUSED, no timeout.
    let c = steam_client::SteamClient::new(
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        "http://127.0.0.1:1",
        steam_client::SteamApiKey::new("SECRETKEY".into()),
    )
    .unwrap();
    let out = c
        .get_owned_games(&steam_client::SteamId64("76561198000000001".into()))
        .await;
    let err_str = format!("{:?}", out.unwrap_err());
    assert!(
        !err_str.contains("SECRETKEY"),
        "network error must not contain the API key; got: {err_str}"
    );
}

// ── Round 2: claimed_id must appear in the openid.signed set ────────────────────────────────

#[tokio::test]
async fn openid_claimed_id_not_in_signed_set_rejected_without_network() {
    // If claimed_id is not among the signed fields, Steam's check_authentication would not
    // recompute the signature over it — so a valid is_valid:true would NOT vouch for the id
    // we extract. Must be rejected before any HTTP. No mock mounted: a network attempt would
    // yield Api(404), not OpenIdRejected.
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    let mut params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return",
    );
    for (k, v) in &mut params {
        if k == "openid.signed" {
            // Signed set WITHOUT claimed_id.
            *v = "signed,op_endpoint,identity,return_to,response_nonce,assoc_handle".into();
        }
    }
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(
        matches!(out, Err(steam_client::SteamError::OpenIdRejected(_))),
        "claimed_id absent from signed set must be rejected pre-network; got: {out:?}"
    );
}

// ── Round 2: near-miss claimed_id shapes all rejected without network ────────────────────────

#[tokio::test]
async fn openid_near_miss_claimed_ids_rejected_without_network() {
    // 16 digits, 18 digits, 17 chars with one embedded non-digit — all must fail the shape
    // pin BEFORE any HTTP (no mock mounted; network attempt → Api(404), not OpenIdRejected).
    let server = wiremock::MockServer::start().await;
    let c = test_openid_client(&server);
    for bad in [
        "https://steamcommunity.com/openid/id/7656119800000001", // 16 digits
        "https://steamcommunity.com/openid/id/765611980000000012", // 18 digits
        "https://steamcommunity.com/openid/id/7656119800000000x", // 17 chars, non-digit
    ] {
        let params = assertion_params(bad, "https://bendobundles.com/api/steam/return");
        let out = c
            .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
            .await;
        assert!(
            matches!(out, Err(steam_client::SteamError::OpenIdRejected(_))),
            "near-miss claimed_id {bad:?} must be rejected pre-network; got: {out:?}"
        );
    }
}

// ── Round 2: strict-line is_valid parse (no substring match) ────────────────────────────────

#[tokio::test]
async fn openid_is_valid_substring_line_is_not_trusted() {
    // A body whose only "is_valid:true" appears embedded in another line must be rejected —
    // pins the trim-exact-line parse against a substring-match regression.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/openid/login"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            "ns:http://specs.openid.net/auth/2.0\nis_valid:false\nx:is_valid:true\n",
        ))
        .mount(&server)
        .await;
    let c = test_openid_client(&server);
    let params = assertion_params(
        "https://steamcommunity.com/openid/id/76561198000000001",
        "https://bendobundles.com/api/steam/return",
    );
    let out = c
        .verify_openid_assertion(&params, "https://bendobundles.com/api/steam/return")
        .await;
    assert!(
        matches!(out, Err(steam_client::SteamError::OpenIdRejected(_))),
        "embedded is_valid:true substring must not be trusted; got: {out:?}"
    );
}

// ── #48: get_app_list via IStoreService/GetAppList/v1 (keyed, paginated) ────────────────────
// Steam removed ISteamApps/GetAppList (404 for everyone, live-observed 2026-07-07); the
// replacement requires the API key and pages via have_more_results/last_appid.

#[tokio::test]
async fn get_app_list_single_page_returns_all_pairs_including_dup_names() {
    // Tier-2 data source for the title-match mapper: dup names INCLUDED — dedup is the
    // mapper's job downstream (unique-only rule), not this method's. Endpoint is KEYED.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetAppList/v1/"))
        .and(wiremock::matchers::query_param("key", "TESTKEY"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"apps":[{"appid":413150,"name":"Stardew Valley","last_modified":1,"price_change_number":2},{"appid":999,"name":"Stardew Valley"},{"appid":602320,"name":"Train Valley 2"}],"have_more_results":false}}"#,
        ))
        .mount(&server)
        .await;
    let out = test_client(&server).get_app_list().await.unwrap();
    assert_eq!(
        out,
        vec![
            (413150u32, "Stardew Valley".to_string()),
            (999u32, "Stardew Valley".to_string()),
            (602320u32, "Train Valley 2".to_string()),
        ]
    );
}

#[tokio::test]
async fn get_app_list_follows_last_appid_cursor_until_exhausted() {
    // Page 1 (no last_appid param) → have_more_results:true + last_appid cursor;
    // page 2 (last_appid=999) → final page with have_more_results ABSENT (Steam omits it
    // on the last page rather than sending false). Results concatenate in order.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetAppList/v1/"))
        .and(wiremock::matchers::query_param("key", "TESTKEY"))
        .and(wiremock::matchers::query_param_is_missing("last_appid"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"apps":[{"appid":10,"name":"Counter-Strike"},{"appid":999,"name":"Stardew Valley"}],"have_more_results":true,"last_appid":999}}"#,
        ))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetAppList/v1/"))
        .and(wiremock::matchers::query_param("key", "TESTKEY"))
        .and(wiremock::matchers::query_param("last_appid", "999"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(
                r#"{"response":{"apps":[{"appid":602320,"name":"Train Valley 2"}]}}"#,
            ),
        )
        .mount(&server)
        .await;
    let out = test_client(&server).get_app_list().await.unwrap();
    assert_eq!(
        out,
        vec![
            (10u32, "Counter-Strike".to_string()),
            (999u32, "Stardew Valley".to_string()),
            (602320u32, "Train Valley 2".to_string()),
        ]
    );
}

#[tokio::test]
async fn get_app_list_stalled_cursor_terminates_with_partial_results() {
    // Loop guard: a page that claims have_more_results:true but repeats the SAME last_appid
    // cursor would loop forever (each request matches the same mock). Must terminate and
    // return what was collected — tier-2 is best-effort; partial data beats a hung sync.
    // The stalled page is fetched once (its apps land in the result) before the
    // non-advancing cursor is detected, hence the duplicate entry below.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetAppList/v1/"))
        .and(wiremock::matchers::query_param_is_missing("last_appid"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"apps":[{"appid":10,"name":"Counter-Strike"}],"have_more_results":true,"last_appid":10}}"#,
        ))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/IStoreService/GetAppList/v1/"))
        .and(wiremock::matchers::query_param("last_appid", "10"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(
            r#"{"response":{"apps":[{"appid":10,"name":"Counter-Strike"}],"have_more_results":true,"last_appid":10}}"#,
        ))
        .mount(&server)
        .await;
    let out = test_client(&server).get_app_list().await.unwrap();
    assert_eq!(
        out,
        vec![
            (10u32, "Counter-Strike".to_string()),
            (10u32, "Counter-Strike".to_string()),
        ]
    );
}

// ── F2: steam_openid_redirect_url percent-encodes & = / in the return_to value ─────────────

#[test]
fn redirect_url_encodes_special_chars() {
    let realm = "https://bendobundles.com";
    // return_to carries &, =, and / chars — they must be percent-encoded so they cannot
    // inject extra OpenID params, split the URL, or enable header injection.
    let return_to = "https://bendobundles.com/api/steam/return?ctx=%2Fl%2Fabc&foo=a=b";
    let url = steam_client::steam_openid_redirect_url(realm, return_to);

    assert!(
        url.starts_with("https://steamcommunity.com/openid/login?"),
        "must start with Steam login endpoint; got: {url}"
    );
    // & in the return_to value → %26 (not a literal & that would inject a new query param)
    assert!(
        url.contains("%26foo"),
        "& in return_to must be encoded as %26; got: {url}"
    );
    // = in the return_to value → %3D
    assert!(
        url.contains("a%3Db"),
        "= in return_to must be encoded as %3D; got: {url}"
    );
    // / in the path → %2F (no URL-split vector)
    assert!(
        url.contains("%2F"),
        "/ in return_to must be encoded as %2F; got: {url}"
    );
}

// ── Storefront reads: get_app_details, get_review_summary, get_recent_reviews ───────────────

const APPDETAILS_FIXTURE: &str = include_str!("fixtures/appdetails-413150-trimmed.json");
const APPREVIEWS_FIXTURE: &str = include_str!("fixtures/appreviews-overall-413150.json");
const APPREVIEWHISTOGRAM_FIXTURE: &str = include_str!("fixtures/appreviewhistogram-413150.json");

// ── get_app_details ──────────────────────────────────────────────────────────

#[tokio::test]
async fn app_details_found_parses_fields() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(detail.app_id, 413150);
    assert_eq!(detail.developers, vec!["ConcernedApe".to_string()]);
    assert_eq!(detail.release_date, Some("Feb 26, 2016".to_string()));
    assert!(
        detail.genres.contains(&"RPG".to_string()),
        "genres must contain RPG; got {:?}",
        detail.genres
    );
    let hls = detail.video_hls_url.expect("video_hls_url must be Some");
    assert!(
        hls.ends_with("hls_264_master.m3u8?t=1754692862"),
        "hls url must end with hls_264_master.m3u8?t=1754692862; got: {hls}"
    );
}

#[tokio::test]
async fn app_details_filters_categories_to_player_mode_allowlist() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    // Real genres in API order, then ONLY the allowlisted top-level player-mode
    // categories (ids 2, 1, 9) in API order. The fixture's 12 other categories —
    // mode variants (Online Co-op, LAN Co-op, Shared/Split Screen…) and store
    // features (Steam Achievements, Steam Cloud, Family Sharing…) — must be gone.
    assert_eq!(
        detail.genres,
        vec![
            "Indie".to_string(),
            "RPG".to_string(),
            "Simulation".to_string(),
            "Single-player".to_string(),
            "Multi-player".to_string(),
            "Co-op".to_string(),
        ],
        "genres must be real genres + allowlisted player modes only"
    );
}

#[tokio::test]
async fn app_details_tolerates_mistyped_category_ids() {
    // Steam types ids loosely across sibling arrays (genres[].id IS a string), so a
    // category id arriving as a string, bool, or missing must never fail the whole
    // appdetails parse — that would permanently un-enrich the app. String ids that
    // parse numerically still count against the allowlist; junk ids just drop the entry.
    let body = r#"{"413150":{"success":true,"data":{
        "name":"Mixed Ids",
        "genres":[{"id":"23","description":"Indie"}],
        "categories":[
            {"id":"2","description":"Single-player"},
            {"id":true,"description":"Steam Achievements"},
            {"description":"Family Sharing"}
        ]
    }}}"#;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(
        detail.genres,
        vec!["Indie".to_string(), "Single-player".to_string()],
        "string id \"2\" must count as allowlisted; junk/missing ids must drop, not fail"
    );
}

#[tokio::test]
async fn app_details_parses_content_descriptors() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(detail.content_descriptor_ids, vec![1, 5]);
    assert_eq!(detail.content_notes, Some("Some nudity.".to_string()));
    // get_app_details never fills tags — enrichment owns them (GetItems).
    assert!(detail.tags.is_empty());
}

#[tokio::test]
async fn app_details_tolerates_missing_content_descriptors() {
    // Fixture WITHOUT the key — most apps. Serde default must yield empties, not a parse error.
    let body = APPDETAILS_FIXTURE.replace(
        r#""content_descriptors": { "ids": [1, 5], "notes": "Some nudity." },"#,
        "",
    );
    assert_ne!(
        body, APPDETAILS_FIXTURE,
        "needle must have matched — fixture line drifted?"
    );
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert!(detail.content_descriptor_ids.is_empty());
    assert_eq!(detail.content_notes, None);
}

#[test]
fn steam_app_detail_blob_backcompat() {
    // A cache blob written before this build (no tags/descriptor fields) must deserialize.
    let old = r#"{"app_id":1,"name":"x","developers":[],"publishers":[],"genres":["RPG"],
        "release_date":null,"short_description":"","header_image":null,
        "video_hls_url":null,"video_thumbnail":null}"#;
    let d: steam_client::SteamAppDetail = serde_json::from_str(old).unwrap();
    assert!(d.tags.is_empty());
    assert!(d.content_descriptor_ids.is_empty());
    assert_eq!(d.content_notes, None);
}

#[tokio::test]
async fn app_details_delisted_is_success_false() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(r#"{"413150":{"success":false}}"#),
        )
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    assert!(
        matches!(result, steam_client::AppDetails::Delisted),
        "expected Delisted; got Found"
    );
}

#[tokio::test]
async fn app_details_rate_limited() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .respond_with(wiremock::ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let out = test_client(&server).get_app_details(413150).await;
    assert!(
        matches!(out, Err(steam_client::SteamError::RateLimited)),
        "expected RateLimited; got {out:?}"
    );
}

// ── get_review_summary ───────────────────────────────────────────────────────

#[tokio::test]
async fn review_summary_parses_overall() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/appreviews/413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPREVIEWS_FIXTURE))
        .mount(&server)
        .await;
    let summary = test_client(&server)
        .get_review_summary(413150)
        .await
        .unwrap();
    assert_eq!(summary.desc, "Overwhelmingly Positive");
    assert_eq!(summary.total_reviews, 460881);
}

#[tokio::test]
async fn review_summary_rate_limited() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/appreviews/413150"))
        .respond_with(wiremock::ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let out = test_client(&server).get_review_summary(413150).await;
    assert!(
        matches!(out, Err(steam_client::SteamError::RateLimited)),
        "expected RateLimited; got {out:?}"
    );
}

// ── get_recent_reviews ───────────────────────────────────────────────────────

#[tokio::test]
async fn recent_reviews_histogram_derived() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/appreviewhistogram/413150"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(APPREVIEWHISTOGRAM_FIXTURE),
        )
        .mount(&server)
        .await;
    let recent = test_client(&server)
        .get_recent_reviews(413150)
        .await
        .unwrap();
    assert_eq!(recent.percent_positive, 98);
    assert_eq!(recent.count, 9200);
}

#[tokio::test]
async fn recent_reviews_rate_limited() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/appreviewhistogram/413150"))
        .respond_with(wiremock::ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let out = test_client(&server).get_recent_reviews(413150).await;
    assert!(
        matches!(out, Err(steam_client::SteamError::RateLimited)),
        "expected RateLimited; got {out:?}"
    );
}

#[tokio::test]
async fn recent_reviews_percent_rounds_not_floors() {
    // Spec pin: percent = round(100*up/(up+down)), not floor.
    // Case: up=2, down=1 → 2/3=66.667% → rounds to 67, floors to 66.
    // Verifies the fix: uses (100*up + total/2) / total integer math.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/appreviewhistogram/413150"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_string(
                r#"{"success":1,"results":{"recent":[{"recommendations_up":2,"recommendations_down":1}]}}"#,
            ),
        )
        .mount(&server)
        .await;
    let recent = test_client(&server)
        .get_recent_reviews(413150)
        .await
        .unwrap();
    assert_eq!(
        recent.percent_positive, 67,
        "66.67% must round to 67, not floor to 66"
    );
    assert_eq!(recent.count, 3);
}

// ── screenshots (issue #61) ──────────────────────────────────────────────────

#[tokio::test]
async fn app_details_parses_screenshots_thumb_and_full_capped_at_10() {
    // Fixture is a REAL captured appdetails response (16 screenshots) — the wire field
    // names (path_thumbnail/path_full) are pinned by capture, not by hand.
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(APPDETAILS_FIXTURE))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(
        detail.screenshots.len(),
        10,
        "16 in the fixture must cap at 10"
    );
    let first = &detail.screenshots[0];
    assert!(
        first
            .thumbnail
            .contains("ss_b887651a93b0525739049eb4194f633de2df75be.600x338"),
        "first thumbnail must be the capture's path_thumbnail; got {}",
        first.thumbnail
    );
    assert!(
        first
            .full
            .contains("ss_b887651a93b0525739049eb4194f633de2df75be.1920x1080"),
        "first full must be the capture's path_full; got {}",
        first.full
    );
}

#[tokio::test]
async fn app_details_missing_screenshots_key_is_empty() {
    // Pre-existing blobs / apps without screenshots: absent key must parse to [], never fail.
    let body = r#"{"413150":{"success":true,"data":{"name":"No Shots"}}}"#;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(detail.screenshots, vec![]);
}

#[tokio::test]
async fn app_details_screenshot_missing_either_tier_is_dropped() {
    // Both URLs or nothing — asymmetric fallbacks would create two sources of truth.
    // Empty strings count as missing (a "" URL would render a phantom slide), and an
    // empty movie hls/thumbnail must collapse to None, not Some("").
    let body = r#"{"413150":{"success":true,"data":{
        "name":"Partial Shots",
        "movies":[{"id":1,"thumbnail":"","hls_h264":""}],
        "screenshots":[
            {"id":0,"path_thumbnail":"https://img.example/a.600x338.jpg","path_full":"https://img.example/a.1920x1080.jpg"},
            {"id":1,"path_thumbnail":"https://img.example/b.600x338.jpg"},
            {"id":2,"path_full":"https://img.example/c.1920x1080.jpg"},
            {"id":3,"path_thumbnail":"","path_full":"https://img.example/d.1920x1080.jpg"}
        ]
    }}}"#;
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/api/appdetails"))
        .and(wiremock::matchers::query_param("appids", "413150"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;
    let result = test_client(&server).get_app_details(413150).await.unwrap();
    let detail = match result {
        steam_client::AppDetails::Found(d) => d,
        steam_client::AppDetails::Delisted => panic!("expected Found, got Delisted"),
    };
    assert_eq!(
        detail.screenshots,
        vec![steam_client::Screenshot {
            thumbnail: "https://img.example/a.600x338.jpg".into(),
            full: "https://img.example/a.1920x1080.jpg".into(),
        }],
        "entries missing either tier must drop, not fail or half-fill"
    );
    assert_eq!(
        detail.video_hls_url, None,
        "empty hls url must be None, not Some(\"\")"
    );
    assert_eq!(detail.video_thumbnail, None, "empty thumbnail must be None");
}

/// parse_release_date: Steam's display formats → ISO date. Full dates parse
/// exact, bare month-year parses to the first of the month, everything else
/// (TBA / Coming soon / empty / garbage) is None.
#[test]
fn parse_release_date_observed_formats() {
    use steam_client::parse_release_date;
    let d = |s: &str| parse_release_date(s).map(|d| d.to_string());
    // full date, EU order (the dominant Steam format)
    assert_eq!(d("12 Nov 2019"), Some("2019-11-12".into()));
    assert_eq!(d("1 Jan 2024"), Some("2024-01-01".into()));
    // full date, US order with comma
    assert_eq!(d("Nov 12, 2019"), Some("2019-11-12".into()));
    // month-year → first of month
    assert_eq!(d("Nov 2019"), Some("2019-11-01".into()));
    // surrounding whitespace tolerated
    assert_eq!(d("  12 Nov 2019 "), Some("2019-11-12".into()));
    // unparseable → None
    assert_eq!(d("Coming soon"), None);
    assert_eq!(d("TBA"), None);
    assert_eq!(d(""), None);
    assert_eq!(d("Q3 2026"), None);
}
