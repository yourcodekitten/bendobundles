//! READ-ONLY live probe of the unofficial humble API. Run by a human, never CI:
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- orders
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- order <gamekey>
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- summary
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- find <name-fragment>
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- capture <gamekey> [outfile]
//!
//! Security: HUMBLE_SESSION is never printed or written to any file.
use humble_client::{HumbleClient, Order, SessionCookie};
use std::collections::HashMap;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let session = std::env::var("HUMBLE_SESSION").expect("set HUMBLE_SESSION");
    let client = HumbleClient::new(
        "https://www.humblebundle.com",
        SessionCookie::new(session.clone()),
    )?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd] if cmd == "orders" => {
            let keys = client.gamekeys().await?;
            println!("{} orders", keys.len());
            for k in keys.iter().take(5) {
                println!("  {k}");
            }
        }
        [cmd, gamekey] if cmd == "order" => {
            let order = client.order(gamekey).await?;
            println!("{} — {} keys", order.bundle_name, order.keys.len());
            for k in &order.keys {
                println!(
                    "  [{}] {} ({}) redeemed={} expired={} giftable={}",
                    k.key_type, k.human_name, k.machine_name, k.redeemed, k.expired, k.giftable
                );
            }
        }
        [cmd] if cmd == "summary" => {
            run_summary(&client).await?;
        }
        [cmd, fragment] if cmd == "find" => {
            run_find(&client, fragment).await?;
        }
        [cmd, gamekey] if cmd == "capture" => {
            run_capture(&session, gamekey, None).await?;
        }
        [cmd, gamekey, outfile] if cmd == "capture" => {
            run_capture(&session, gamekey, Some(outfile.as_str())).await?;
        }
        _ => {
            eprintln!(
                "usage:\n  probe orders\n  probe order <gamekey>\n  probe summary\n  probe find <name-fragment>\n  probe capture <gamekey> [outfile]"
            );
        }
    }
    Ok(())
}

/// Fetch every order in the library, politely: sleeps 300 ms between fetches,
/// prints `progress_verb N/total` progress to stderr every 25 orders, and logs
/// any single fetch failure as WARN then skips it — partial data beats an
/// aborted run. `on_start` receives the order count before the loop (for an
/// intro line); `f` is called for each cleanly-fetched order. Returns the
/// (total orders, fetch failures) counts.
async fn for_each_order(
    client: &HumbleClient,
    progress_verb: &str,
    on_start: impl FnOnce(usize),
    mut f: impl FnMut(&str, Order),
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let gamekeys = client.gamekeys().await?;
    let total = gamekeys.len();
    on_start(total);

    let mut fail_count = 0usize;
    for (i, gamekey) in gamekeys.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if (i + 1) % 25 == 0 {
            eprintln!("{progress_verb} {}/{total}...", i + 1);
        }

        match client.order(gamekey).await {
            Ok(order) => f(gamekey, order),
            Err(e) => {
                eprintln!("WARN {gamekey}: {e}");
                fail_count += 1;
            }
        }
    }

    Ok((total, fail_count))
}

/// Fetch every order, aggregate stats, and print a coverage report.
async fn run_summary(client: &HumbleClient) -> Result<(), Box<dyn std::error::Error>> {
    let mut total_keys = 0usize;
    let mut key_type_counts: HashMap<String, usize> = HashMap::new();
    let mut redeemed_count = 0usize;
    let mut expired_count = 0usize;
    let mut giftable_count = 0usize;
    let mut zero_key_bundles: Vec<String> = Vec::new();
    let mut all_bundles: Vec<(String, usize)> = Vec::new(); // (bundle_name, giftable_count)

    let (total, fail_count) = for_each_order(
        client,
        "fetched",
        |total| eprintln!("fetching {total} orders..."),
        |_gamekey, order| {
            let n_keys = order.keys.len();
            total_keys += n_keys;
            let mut bundle_giftable = 0usize;
            for k in &order.keys {
                *key_type_counts.entry(k.key_type.clone()).or_insert(0) += 1;
                if k.redeemed {
                    redeemed_count += 1;
                }
                if k.expired {
                    expired_count += 1;
                }
                if k.giftable {
                    giftable_count += 1;
                    bundle_giftable += 1;
                }
            }
            if n_keys == 0 {
                zero_key_bundles.push(order.bundle_name.clone());
            }
            all_bundles.push((order.bundle_name, bundle_giftable));
        },
    )
    .await?;

    // Sort bundles by giftable desc for top-10
    all_bundles.sort_by_key(|b| std::cmp::Reverse(b.1));

    // Sort key_type counts desc
    let mut key_type_sorted: Vec<(String, usize)> = key_type_counts.into_iter().collect();
    key_type_sorted.sort_by_key(|b| std::cmp::Reverse(b.1));

    println!("=== humble library coverage summary ===");
    println!("total orders:        {total}");
    println!("  fetch failures:    {fail_count}");
    println!("total key entries:   {total_keys}");
    println!("  redeemed:          {redeemed_count}");
    println!("  expired:           {expired_count}");
    println!("  giftable:          {giftable_count}");
    println!();
    println!("key_type distribution (desc):");
    for (kt, n) in &key_type_sorted {
        println!("  {kt:<30} {n}");
    }
    println!();
    let zero_count = zero_key_bundles.len();
    println!("orders with zero key entries: {zero_count} (candidate Choice/DRM-free bundles)");
    for name in zero_key_bundles.iter().take(10) {
        println!("  {name}");
    }
    if zero_count > 10 {
        println!("  ... and {} more", zero_count - 10);
    }
    println!();
    println!("top 10 bundles by giftable key count:");
    for (name, n) in all_bundles.iter().take(10) {
        println!("  {n:>4}  {name}");
    }

    Ok(())
}

/// Search every order for bundles whose name contains the given fragment
/// (case-insensitive substring match). Matches are printed immediately to
/// stdout; ends with a match count line.
async fn run_find(client: &HumbleClient, fragment: &str) -> Result<(), Box<dyn std::error::Error>> {
    let fragment_lower = fragment.to_lowercase();
    let mut match_count = 0usize;

    for_each_order(
        client,
        "searched",
        |total| eprintln!("searching {total} orders for '{fragment}'..."),
        |gamekey, order| {
            if order.bundle_name.to_lowercase().contains(&fragment_lower) {
                println!("{gamekey}  {}", order.bundle_name);
                match_count += 1;
            }
        },
    )
    .await?;

    let plural = if match_count == 1 { "match" } else { "matches" };
    println!("{match_count} {plural}");

    Ok(())
}

/// Fetch a raw order response as `serde_json::Value` using a probe-local reqwest
/// client — the library's typed client stays typed; no lib changes needed.
/// The session value is used only in the Cookie header and never logged.
async fn raw_get(
    session: &str,
    url: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let resp = http
        .get(url)
        .header("Cookie", format!("_simpleauth_sess={session}"))
        .header("X-Requested-By", "hb_android_app")
        .send()
        .await?;
    let status = resp.status().as_u16();
    if status != 200 {
        return Err(format!("HTTP {status}").into());
    }
    let bytes = resp.bytes().await?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Recursively walk a JSON value and replace the VALUE of any field named
/// `redeemed_key_val` or `giftkey` with the string "REDACTED".
/// Steam keys and gift tokens must never land on disk in plaintext.
fn redact(val: &mut serde_json::Value) {
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == "redeemed_key_val" || k == "giftkey" {
                    *v = serde_json::Value::String("REDACTED".to_string());
                } else {
                    redact(v);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact(v);
            }
        }
        _ => {}
    }
}

/// Capture a redacted raw order response to disk as contract evidence.
///
/// Writes pretty-printed JSON to `outfile` (default `./captures/<gamekey>.json`).
/// Prints the destination path and a one-line shape hint (tpk count, or a
/// drift warning if `tpkd_dict.all_tpks` is absent).
async fn run_capture(
    session: &str,
    gamekey: &str,
    outfile: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let url = format!("https://www.humblebundle.com/api/v1/order/{gamekey}?all_tpkds=true");
    let mut val = raw_get(session, &url).await?;
    redact(&mut val);

    let path = match outfile {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let dir = std::path::PathBuf::from("captures");
            std::fs::create_dir_all(&dir)?;
            dir.join(format!("{gamekey}.json"))
        }
    };
    // Ensure any explicit parent directory exists too
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let pretty = serde_json::to_string_pretty(&val)?;
    std::fs::write(&path, &pretty)?;

    let tpk_count = val
        .get("tpkd_dict")
        .and_then(|d| d.get("all_tpks"))
        .and_then(|a| a.as_array())
        .map(|a| a.len());
    let shape_hint = match tpk_count {
        Some(n) => format!("{n} tpks in tpkd_dict.all_tpks"),
        None => "MISSING tpkd_dict.all_tpks — shape drift!".to_string(),
    };

    println!("wrote {}", path.display());
    println!("{shape_hint}");

    Ok(())
}
