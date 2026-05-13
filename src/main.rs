use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tokio::time::{sleep, Duration};
use dotenv::dotenv;
use std::env;
use base64::{engine::general_purpose, Engine as _};
use mongodb::{Client, Collection};
use bson::doc;
use chrono::Utc;

#[derive(Debug, Deserialize)]
struct TransactionsResponse {
    transactions: Vec<Transaction>,
}

#[derive(Debug, Deserialize, Clone)]
struct Transaction {
    hash: String,
    account: String,
    in_msg: Option<Message>,
    description: Option<Description>,
    now: i64,
}

#[derive(Debug, Deserialize, Clone)]
struct Message {
    opcode: Option<String>,
    source: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
struct Description {
    action: Option<Action>,
}

#[derive(Debug, Deserialize, Clone)]
struct Action {
    success: bool,
}

#[derive(Debug, Deserialize)]
struct JettonMetadataResponse {
    metadata: std::collections::HashMap<String, MetadataEntry>,
}

#[derive(Debug, Deserialize)]
struct MetadataEntry {
    token_info: Option<Vec<TokenInfo>>,
}

#[derive(Debug, Deserialize)]
struct TokenInfo {
    name: Option<String>,
    symbol: Option<String>,
}

// MongoDB Model
#[derive(Debug, Serialize, Deserialize)]
struct TokenTrade {
    token_address: String,
    token_name: String,
    token_symbol: String,
    buy_hash: String,
    amount_ton_spent: f64,
    buy_timestamp_ms: i64,
    status: String,
}

async fn is_new_launch(client: &reqwest::Client, address: &str) -> bool {
    let url = format!("https://toncenter.com/api/v3/transactions?account={}&limit=10", address);
    if let Ok(resp) = client.get(url).send().await {
        if let Ok(data) = resp.json::<TransactionsResponse>().await {
            return data.transactions.len() <= 5;
        }
    }
    true
}

async fn get_account_balance(client: &reqwest::Client, address: &str) -> String {
    let url = format!("https://toncenter.com/api/v2/getAddressInformation?address={}", address);
    if let Ok(resp) = client.get(url).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(result) = json.get("result") {
                let balance_val = result.get("balance");
                let balance_nano = if let Some(s) = balance_val.and_then(|v| v.as_str()) {
                    s.parse::<u128>().ok()
                } else if let Some(n) = balance_val.and_then(|v| v.as_u64()) {
                    Some(n as u128)
                } else {
                    None
                };

                if let Some(nano) = balance_nano {
                    return format!("{:.2} TON", (nano as f64) / 1_000_000_000.0);
                }
            }
        }
    }

    let url_tonapi = format!("https://tonapi.io/v2/accounts/{}", address);
    if let Ok(resp) = client.get(url_tonapi).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(balance) = json.get("balance").and_then(|v| v.as_u64()) {
                return format!("{:.2} TON", (balance as f64) / 1_000_000_000.0);
            }
        }
    }

    "Unknown".to_string()
}

async fn get_jetton_info(client: &reqwest::Client, address: &str) -> (String, String) {
    let url = format!("https://toncenter.com/api/v3/jetton/masters?address={}", address);
    if let Ok(resp) = client.get(url).send().await {
        if let Ok(data) = resp.json::<JettonMetadataResponse>().await {
            for (_, entry) in data.metadata {
                if let Some(info_list) = entry.token_info {
                    if let Some(info) = info_list.first() {
                        let name = info.name.clone().unwrap_or_else(|| "Unknown".to_string());
                        let symbol = info.symbol.clone().unwrap_or_else(|| "???".to_string());
                        if name != "Unknown" {
                            return (name, symbol);
                        }
                    }
                }
            }
        }
    }

    let url_tonapi = format!("https://tonapi.io/v2/jettons/{}", address);
    if let Ok(resp) = client.get(url_tonapi).send().await {
        if let Ok(json) = resp.json::<serde_json::Value>().await {
            if let Some(metadata) = json.get("metadata") {
                let name = metadata.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string();
                let symbol = metadata.get("symbol").and_then(|v| v.as_str()).unwrap_or("???").to_string();
                return (name, symbol);
            }
        }
    }

    ("Unknown".to_string(), "???".to_string())
}

async fn send_telegram(client: &reqwest::Client, token: &str, chat_id: &str, text: &str, reply_to: Option<i64>) -> Result<i64, Box<dyn std::error::Error>> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
    let mut body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown",
        "disable_web_page_preview": true
    });

    if let Some(msg_id) = reply_to {
        body["reply_to_message_id"] = serde_json::json!(msg_id);
    }

    let resp = client.post(url)
        .json(&body)
        .send()
        .await?;

    let json: serde_json::Value = resp.json().await?;
    let message_id = json["result"]["message_id"].as_i64().unwrap_or(0);
    Ok(message_id)
}

async fn sniper_worker(db: Collection<TokenTrade>) {
    println!("🔫 Sniper Worker: Monitoring all shards for mints...");

    let bot_token = env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
    let chat_id = env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
    let buy_ton: f64 = env::var("BUY_TON").unwrap_or_else(|_| "2".to_string()).parse().unwrap_or(2.0);

    let client = reqwest::Client::new();
    let base_url = "https://toncenter.com/api/v3/transactions?limit=500&sort=desc&workchain=0";

    let mut processed_hashes = HashSet::new();
    let mut hash_queue = std::collections::VecDeque::new();
    let max_hashes = 20000;

    let mint_opcodes = vec!["0x642b7d07", "0xcc1a97aa", "0x00000015", "0x16740000"];

    loop {
        let response = match client.get(base_url).send().await {
            Ok(resp) => resp,
            Err(_) => {
                sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        if response.status().as_u16() == 429 {
            println!("🛑 429 - Waiting for cooldown...");
            sleep(Duration::from_secs(10)).await;
            continue;
        }

        let data: TransactionsResponse = match response.json().await {
            Ok(d) => d,
            Err(_) => {
                sleep(Duration::from_secs(1)).await;
                continue;
            }
        };

        let mut found_mints = Vec::new();
        for tx in data.transactions {
            if !processed_hashes.contains(&tx.hash) {
                processed_hashes.insert(tx.hash.clone());
                hash_queue.push_back(tx.hash.clone());
                if hash_queue.len() > max_hashes {
                    if let Some(old) = hash_queue.pop_front() {
                        processed_hashes.remove(&old);
                    }
                }

                let success = tx.description.as_ref()
                    .and_then(|d| d.action.as_ref())
                    .map(|a| a.success)
                    .unwrap_or(false);

                if success {
                    if let Some(ref msg) = tx.in_msg {
                        if let Some(ref opcode) = msg.opcode {
                            if mint_opcodes.contains(&opcode.as_str()) {
                                found_mints.push(tx.clone());
                            }
                        }
                    }
                }
            }
        }

        for tx in found_mints {
            if !is_new_launch(&client, &tx.account).await {
                continue;
            }

            let (name, symbol) = get_jetton_info(&client, &tx.account).await;
            
            let time = chrono::DateTime::from_timestamp(tx.now, 0)
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_default();

            let hex_hash = if let Ok(decoded) = general_purpose::STANDARD.decode(&tx.hash) {
                hex::encode(decoded)
            } else {
                tx.hash.clone()
            };

            let minter_addr = tx.in_msg.as_ref().and_then(|m| m.source.clone()).unwrap_or_else(|| "Unknown".to_string());
            let minter_balance = if minter_addr != "Unknown" {
                get_account_balance(&client, &minter_addr).await
            } else {
                "Unknown".to_string()
            };

            println!("✨ NEW MINT: {} ({}) | Minter: {} | Balance: {}", name, symbol, minter_addr, minter_balance);

            let message1 = format!(
                "💎 *New Jetton Minted!*\n\n\
                 🏷️ *Name:* `{}`\n\
                 🔤 *Symbol:* `{}`\n\
                 🕒 *Time:* {}\n\
                 📦 *Opcode:* `{}`\n\n\
                 📍 *Master:* [Explorer](https://tonviewer.com/{})\n\
                 🔗 *Hash:* [Transaction](https://tonviewer.com/transaction/{})",
                name, symbol, time, 
                tx.in_msg.as_ref().and_then(|m| m.opcode.clone()).unwrap_or_default(),
                tx.account, hex_hash
            );

            match send_telegram(&client, &bot_token, &chat_id, &message1, None).await {
                Ok(msg_id) => {
                    let message2 = format!(
                        "👤 *Minter Info*\n\n\
                         💰 *Balance:* `{}`\n\
                         📍 *Address:* `{}`\n\
                         🔗 [View on Explorer](https://tonviewer.com/{})",
                        minter_balance, minter_addr, minter_addr
                    );

                    let _ = send_telegram(&client, &bot_token, &chat_id, &message2, Some(msg_id)).await;
                }
                Err(e) => eprintln!("❌ Telegram error (Msg 1): {}", e),
            }

            // SIMULATE BUY
            println!("🛒 [SIMULATION] Purchasing {} for {} TON...", symbol, buy_ton);
            
            let trade = TokenTrade {
                token_address: tx.account.clone(),
                token_name: name.clone(),
                token_symbol: symbol.clone(),
                buy_hash: hex_hash.clone(),
                amount_ton_spent: buy_ton,
                buy_timestamp_ms: Utc::now().timestamp_millis(),
                status: "PENDING".to_string(),
            };

            if let Err(e) = db.insert_one(trade, None).await {
                eprintln!("❌ Failed to insert trade into MongoDB: {}", e);
            } else {
                println!("✅ [SIMULATION] Trade recorded in MongoDB.");
                let alert = format!("🛒 *SIMULATION: Buy Executed*\n\nBought {} TON of `{}`.", buy_ton, symbol);
                let _ = send_telegram(&client, &bot_token, &chat_id, &alert, None).await;
            }
        }

        sleep(Duration::from_secs(2)).await;
    }
}

async fn seller_worker(db: Collection<TokenTrade>) {
    println!("⚖️ Seller Worker: Checking for tokens to sell...");

    let bot_token = env::var("TELEGRAM_BOT_TOKEN").unwrap_or_default();
    let chat_id = env::var("TELEGRAM_CHAT_ID").unwrap_or_default();
    let autosell_ms: i64 = env::var("AUTOSELL_MS").unwrap_or_else(|_| "300000".to_string()).parse().unwrap_or(300000);
    
    let client = reqwest::Client::new();

    loop {
        let now = Utc::now().timestamp_millis();
        let target_time = now - autosell_ms;

        let query = doc! {
            "status": "PENDING",
            "buy_timestamp_ms": { "$lte": target_time }
        };

        if let Ok(mut cursor) = db.find(query, None).await {
            use futures_util::StreamExt;
            while let Some(result) = cursor.next().await {
                if let Ok(trade) = result {
                    println!("📉 [SIMULATION] Selling {} ({}) after delay...", trade.token_name, trade.token_symbol);
                    
                    // SIMULATE SELL
                    let update_query = doc! { "token_address": &trade.token_address, "status": "PENDING" };
                    let update = doc! { "$set": { "status": "SOLD" } };
                    
                    if let Ok(_) = db.update_one(update_query, update, None).await {
                        println!("✅ [SIMULATION] Trade marked as SOLD in MongoDB.");
                        let alert = format!("📉 *SIMULATION: Sell Executed*\n\nSold `{}` after delay.", trade.token_symbol);
                        let _ = send_telegram(&client, &bot_token, &chat_id, &alert, None).await;
                    }
                }
            }
        }

        // Check every 10 seconds
        sleep(Duration::from_secs(10)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();
    
    println!("🚀 Initializing TON Sniper & Seller Framework...");

    // Connect to MongoDB
    let mongo_uri = env::var("MONGODB_CONNECTION_STRING").expect("MONGODB_CONNECTION_STRING not found in .env");
    let mongo_client = Client::with_uri_str(&mongo_uri).await?;
    let db = mongo_client.database("crypto0");
    let trades_collection = db.collection::<TokenTrade>("trades");

    println!("✅ Connected to MongoDB.");

    // Spawn Workers
    let sniper_task = tokio::spawn(sniper_worker(trades_collection.clone()));
    let seller_task = tokio::spawn(seller_worker(trades_collection.clone()));

    // Wait for both to complete (they shouldn't)
    let _ = tokio::join!(sniper_task, seller_task);

    Ok(())
}
