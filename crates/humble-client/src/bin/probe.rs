//! READ-ONLY live probe of the unofficial humble API. Run by a human, never CI:
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- orders
//!   HUMBLE_SESSION='<cookie>' cargo run -p humble-client --features probe --bin probe -- order <gamekey>
use humble_client::{HumbleClient, SessionCookie};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cookie = std::env::var("HUMBLE_SESSION").expect("set HUMBLE_SESSION");
    let client = HumbleClient::new("https://www.humblebundle.com", SessionCookie::new(cookie))?;
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
        _ => eprintln!("usage: probe orders | probe order <gamekey>"),
    }
    Ok(())
}
