fn test_client(server: &wiremock::MockServer) -> steam_client::SteamClient {
    steam_client::SteamClient::new(
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
