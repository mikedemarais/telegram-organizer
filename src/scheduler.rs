use tokio::time::{sleep, Duration};
use log::{info, error};
use grammers_client::Client;
use crate::{telegram, database, ai};
use std::collections::HashMap;
use chrono::{DateTime, Utc};
use textwrap;

/// Run the periodic data fetch and analysis cycle every 30 minutes.
pub async fn run_schedule(client: &Client, conn: &mut rusqlite::Connection) -> Result<(), Box<dyn std::error::Error>> {
    let interval = Duration::from_secs(1800);  // 30 minutes
    loop {
        // 1. Fetch all current chats (dialogs) from Telegram
        let chat_list = match telegram::fetch_dialogs(client).await {
            Ok(list) => list,
            Err(e) => {
                error!("Failed to fetch dialogs: {}", e);
                // Wait and retry on next cycle
                sleep(interval).await;
                continue;
            }
        };
        // 2. Save/update chats in database (preserve existing AI insights)
        for chat in &chat_list {
            if let Err(e) = database::save_chat(conn, chat) {
                error!("DB error saving chat {}: {}", chat.title, e);
            }
            
            // Fetch and save member information
            match telegram::fetch_chat_members(client, chat).await {
                Ok(members) => {
                    for (user_id, name, username, bio) in members {
                        if let Err(e) = database::save_member(conn, &chat.peer_id, user_id, &name, username.as_deref(), bio.as_deref(), 0) {
                            error!("Failed to save member {} for chat {}: {}", name, chat.title, e);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to fetch members for chat {}: {}", chat.title, e);
                }
            }
        }
        // 3. For each chat, fetch new messages and process AI analysis
        // We'll collect categories for duplicate detection
        let mut categories: Vec<(String, String)> = Vec::new();  // (category, peer_id)
        for chat in &chat_list {
            // Get last processed message ID for this chat from the database
            let last_id = get_last_message_id(conn, &chat.peer_id).unwrap_or(0);
            match telegram::fetch_new_messages(client, chat, if last_id > 0 { Some(last_id) } else { None }).await {
                Ok(new_msgs) => {
                    if !new_msgs.is_empty() {
                        info!("{} new messages in chat \"{}\"", new_msgs.len(), chat.title);
                    }
                    // Save new messages to database
                    if let Err(e) = database::save_messages(conn, &chat.peer_id, &new_msgs) {
                        error!("DB error saving messages for {}: {}", chat.title, e);
                    }
                    // Determine if we should run AI analysis:
                    // If chat has no category yet, or new messages arrived (which might change urgency or context).
                    let chat_category = get_chat_category(conn, &chat.peer_id);
                    if chat_category.is_none() || !new_msgs.is_empty() {
                        // Prepare message history for context: fetch last 20 messages from DB (including newly added).
                        let recent_msgs = get_recent_messages(conn, &chat.peer_id, 20)?;
                        // Run AI analysis on this chat's content
                        match ai::analyze_chat(&chat.title, &recent_msgs).await {
                            Ok((category, suggested_name, urgent_ids)) => {
                                info!("Chat \"{}\": category=\"{}\", suggested_name=\"{}\"", chat.title, category, suggested_name);
                                // Mark urgent messages in DB
                                if let Err(e) = database::mark_urgent(conn, &chat.peer_id, &urgent_ids) {
                                    error!("Failed to mark urgent messages for {}: {}", chat.title, e);
                                }
                                // We don't decide duplicate here; just store category and suggestion
                                if let Err(e) = database::update_chat_analysis(conn, &chat.peer_id, &category, &suggested_name, false) {
                                    error!("Failed to update analysis for {}: {}", chat.title, e);
                                }
                                categories.push((category, chat.peer_id.clone()));
                            }
                            Err(e) => {
                                error!("AI analysis failed for chat {}: {}", chat.title, e);
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Error fetching messages for chat {}: {}", chat.title, e);
                }
            }
        }
        // 4. Detect duplicate chats by category similarity
        mark_duplicates(conn, &mut categories)?;
        // Sleep until next cycle
        info!("Cycle complete. Next check in 30 minutes.");
        sleep(interval).await;
    }
}

/// Helper: get the last processed message ID for a chat from the DB.
fn get_last_message_id(conn: &rusqlite::Connection, chat_peer: &str) -> Option<i32> {
    let mut stmt = conn.prepare("SELECT MAX(msg_id) FROM messages WHERE chat_peer = ?1;").ok()?;
    let result = stmt.query_row([chat_peer], |row| row.get::<_, Option<i32>>(0));
    match result {
        Ok(opt) => opt,
        Err(_) => None,
    }
}

/// Helper: get recent messages for a chat from DB, up to `limit` count, sorted by ascending date.
fn get_recent_messages(conn: &rusqlite::Connection, chat_peer: &str, limit: usize) 
    -> Result<Vec<telegram::MessageInfo>, Box<dyn std::error::Error>> 
{
    let mut stmt = conn.prepare(
        "SELECT msg_id, date, text FROM messages 
         WHERE chat_peer = ?1 
         ORDER BY msg_id DESC 
         LIMIT ?2;"
    )?;
    let limit_str = limit.to_string();
    let rows = stmt.query_map([chat_peer, &limit_str], |row| {
        Ok(telegram::MessageInfo {
            msg_id: row.get(0)?,
            date: row.get(1)?,
            text: row.get(2)?,
        })
    })?;
    let mut messages = Vec::new();
    for result in rows {
        messages.push(result?);
    }
    // The query gave descending by msg_id, reverse to ascending chronological order
    messages.reverse();
    Ok(messages)
}

/// Helper: get the current category for a chat from DB.
fn get_chat_category(conn: &rusqlite::Connection, chat_peer: &str) -> Option<String> {
    let mut stmt = conn.prepare("SELECT category FROM chats WHERE peer_id = ?1;").ok()?;
    let result = stmt.query_row([chat_peer], |row| row.get::<_, Option<String>>(0));
    match result {
        Ok(opt) => opt,
        Err(_) => None,
    }
}

/// Determine duplicate-topic chats based on categories. 
/// If multiple chats share the same category label (case-insensitive), mark them as duplicates.
fn mark_duplicates(conn: &mut rusqlite::Connection, categories: &mut Vec<(String, String)>) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    if categories.is_empty() {
        return Ok(());
    }

    // First, collect all categories (including existing ones) into a HashMap
    let mut cat_map: HashMap<String, Vec<String>> = HashMap::new();
    
    // Add categories from current cycle
    for (cat, pid) in categories.drain(..) {
        let key = cat.to_lowercase();
        cat_map.entry(key).or_default().push(pid);
    }
    
    // Add existing categories from database
    {
        let mut stmt = conn.prepare("SELECT category, peer_id FROM chats WHERE category IS NOT NULL;")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?.to_lowercase(), row.get::<_, String>(1)?))
        })?;
        
        for res in rows {
            let (cat, pid) = res?;
            cat_map.entry(cat).or_default().push(pid);
        }
    }

    // Now update duplicate flags
    let tx = conn.transaction()?;
    
    // Reset all flags first
    tx.execute("UPDATE chats SET duplicate = 0;", [])?;
    
    // Set duplicate flag for chats in categories with multiple entries
    for (_cat, peers) in cat_map {
        if peers.len() > 1 {
            for peer_id in peers {
                tx.execute("UPDATE chats SET duplicate = 1 WHERE peer_id = ?1;", [&peer_id])?;
            }
        }
    }
    
    tx.commit()?;
    Ok(())
}

/// Generate a report of all chats and any urgent messages, printing to stdout.
pub fn print_report(conn: &rusqlite::Connection) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Telegram Chats Report ===");
    let mut stmt = conn.prepare(
        "SELECT peer_id, name, category, suggested_name, duplicate FROM chats ORDER BY name COLLATE NOCASE;"
    )?;
    let chat_rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?, // peer_id
            row.get::<_, String>(1)?, // name
            row.get::<_, Option<String>>(2)?, // category (nullable)
            row.get::<_, Option<String>>(3)?, // suggested_name (nullable)
            row.get::<_, i32>(4)?      // duplicate flag as 0/1
        ))
    })?;
    for chat_result in chat_rows {
        let (peer_id, name, category, suggested, duplicate_flag) = chat_result?;
        let category_str = category.unwrap_or_else(|| "Uncategorized".into());
        let suggested_str = suggested.unwrap_or_else(|| "-".into());
        let duplicate_str = if duplicate_flag != 0 { "Yes" } else { "No" };
        println!("\nChat: {}{}", name, if duplicate_flag != 0 { " (Duplicate Topic)" } else { "" });
        println!(" - Category: {}", category_str);
        println!(" - Suggested Name: {}", suggested_str);
        println!(" - Duplicate: {}", duplicate_str);
        
        // Print member information
        println!("\n Members:");
        if let Ok(members) = database::get_chat_members(conn, &peer_id) {
            for (_user_id, name, username, bio) in members {
                println!("   * {} (@{})", name, username.unwrap_or_else(|| "-".to_string()));
                if let Some(bio_text) = bio {
                    // Indent and wrap bio text for better readability
                    for line in textwrap::wrap(&bio_text, 60) {
                        println!("     Bio: {}", line);
                    }
                }
            }
        }
        
        // Fetch urgent messages for this chat
        let mut msg_stmt = conn.prepare(
            "SELECT date, text FROM messages 
             WHERE chat_peer = ?1 AND urgent = 1 ORDER BY date ASC;"
        )?;
        let msg_iter = msg_stmt.query_map([peer_id], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
        })?;
        for msg in msg_iter {
            let (ts, text) = msg?;
            // Format timestamp to human-readable using chrono
            let dt = DateTime::<Utc>::from_timestamp(ts as i64, 0)
                .expect("Invalid timestamp");
            let datetime_str = dt.format("%Y-%m-%d %H:%M:%S").to_string();
            let snippet = if text.len() > 50 { &text[0..50] } else { &text };
            println!("   * [URGENT @ {}] {}", datetime_str, snippet);
        }
    }
    println!("\nEnd of report.");
    Ok(())
} 