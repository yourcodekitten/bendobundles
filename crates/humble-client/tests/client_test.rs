use humble_client::{HumbleClient, SessionCookie};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    serde_json::from_str(&raw).unwrap()
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
