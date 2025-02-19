use libsql::{Builder, Connection};
use chrono::{Utc, DateTime};
use log::error;
use crate::telegram::{ChatInfo, MessageInfo};

// Helper: Convert a Vec<f32> to a blob (Vec<u8>) in little-endian format.
fn embedding_to_blob(embedding: &Vec<f32>) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut blob = Vec::with_capacity(embedding.len() * 4);
    for f in embedding {
        blob.extend(&f.to_le_bytes());
    }
    Ok(blob)
}

/// Initialize the libSQL database and create tables if they don't exist.
/// The schema now includes the embedding column in the chat_messages table.
pub async fn init_db(path: &str) -> Result<Connection, Box<dyn std::error::Error>> {
    // Build the local libSQL database asynchronously
    let db = Builder::new_local(path).build().await?;
    let mut conn = db.connect()?;
    
    // Execute the initialization SQL batch (async execution)
    conn.execute_batch(r#"
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS chats (
            peer_id        TEXT PRIMARY KEY,
            type           TEXT,
            tg_id          INTEGER,
            name           TEXT,
            access_hash    INTEGER,
            category       TEXT,
            suggested_name TEXT,
            duplicate      BOOLEAN DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS chat_messages (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            chat_peer  TEXT,
            msg_id     INTEGER,
            date       INTEGER,
            text       TEXT,
            urgent     BOOLEAN DEFAULT 0,
            embedding  F32_BLOB(1024),
            UNIQUE(chat_peer, msg_id) ON CONFLICT IGNORE,
            FOREIGN KEY(chat_peer) REFERENCES chats(peer_id)
        );

        CREATE INDEX IF NOT EXISTS idx_chat_messages_embedding 
            ON chat_messages(libsql_vector_idx(embedding));
        
        CREATE TABLE IF NOT EXISTS users (
            user_id    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            username   TEXT,
            bio        TEXT,
            last_seen  INTEGER,
            UNIQUE(user_id)
        );

        CREATE TABLE IF NOT EXISTS chat_members (
            chat_peer  TEXT,
            user_id    INTEGER,
            joined_at  INTEGER,
            PRIMARY KEY (chat_peer, user_id),
            FOREIGN KEY(chat_peer) REFERENCES chats(peer_id),
            FOREIGN KEY(user_id) REFERENCES users(user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_chat_members_user ON chat_members(user_id);
        CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
    "#).await?;
    
    Ok(conn)
}

/// Insert or update chat info in the database (without touching AI fields).
pub async fn save_chat(conn: &mut Connection, chat: &ChatInfo) -> Result<(), Box<dyn std::error::Error>> {
    conn.execute(
        "INSERT INTO chats (peer_id, type, tg_id, name, access_hash)\n         VALUES (?1, ?2, ?3, ?4, ?5)\n         ON CONFLICT(peer_id) DO UPDATE SET \n            name = excluded.name, access_hash = excluded.access_hash;",
        &[&chat.peer_id,
          &match chat.kind {
              crate::telegram::ChatKind::Group => "Group",
              crate::telegram::ChatKind::Channel => "Channel",
          },
          &chat.tg_id,
          &chat.title,
          &chat.access_hash.unwrap_or(0)],
    ).await?;
    Ok(())
}

/// Save a batch of new messages for a chat, including their embeddings.
/// Each tuple contains a MessageInfo and its corresponding embedding vector.
pub async fn save_messages(conn: &mut Connection, chat_peer: &str, messages: &[(MessageInfo, Vec<f32>)]) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    if messages.is_empty() {
        return Ok(());  // nothing to do
    }
    let mut tx = conn.transaction().await?;
    for (msg, embedding) in messages {
        let emb_blob = embedding_to_blob(embedding)?;
        tx.execute(
            "INSERT OR IGNORE INTO chat_messages (chat_peer, msg_id, date, text, embedding) \n             VALUES (?1, ?2, ?3, ?4, ?5);",
            &[&chat_peer, &msg.msg_id, &msg.date, &msg.text, &emb_blob],
        ).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Update chat analysis results (category, suggested name, duplicate flag) for a given chat.
pub async fn update_chat_analysis(conn: &mut Connection, peer_id: &str, category: &str, suggested_name: &str, duplicate: bool) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    conn.execute(
        "UPDATE chats SET category = ?1, suggested_name = ?2, duplicate = ?3 WHERE peer_id = ?4;",
        &[&category, &suggested_name, &(duplicate as i32), &peer_id],
    ).await?;
    Ok(())
}

/// Mark specific messages as urgent in the database.
pub async fn mark_urgent(conn: &mut Connection, chat_peer: &str, msg_ids: &[i32]) -> Result<(), Box<dyn std::error::Error>> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let mut tx = conn.transaction().await?;
    for &mid in msg_ids {
        tx.execute(
            "UPDATE chat_messages SET urgent = 1 WHERE chat_peer = ?1 AND msg_id = ?2;",
            &[&chat_peer, &mid],
        ).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Generate a report of all chats and any urgent messages, printing to stdout.
pub async fn print_report(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Telegram Chats Report ===");
    let mut stmt = conn.prepare(
        "SELECT peer_id, name, category, suggested_name, duplicate FROM chats ORDER BY name COLLATE NOCASE;"
    ).await?;
    let mut rows = stmt.query(&[]).await?;
    while let Some(row) = rows.next().await? {
        let peer_id: String = row.get(0)?;
        let name: String = row.get(1)?;
        let category: Option<String> = row.get(2)?;
        let suggested: Option<String> = row.get(3)?;
        let duplicate_flag: i32 = row.get(4)?;
        let category_str = category.unwrap_or_else(|| "Uncategorized".into());
        let suggested_str = suggested.unwrap_or_else(|| "-".into());
        let duplicate_str = if duplicate_flag != 0 { "Yes" } else { "No" };
        println!("\nChat: {}{}", name, if duplicate_flag != 0 { " (Duplicate Topic)" } else { "" });
        println!(" - Category: {}", category_str);
        println!(" - Suggested Name: {}", suggested_str);
        println!(" - Duplicate: {}", duplicate_str);

        let mut msg_stmt = conn.prepare(
            "SELECT date, text FROM chat_messages \n             WHERE chat_peer = ?1 AND urgent = 1 ORDER BY date ASC;"
        ).await?;
        let mut msg_rows = msg_stmt.query(&[&peer_id]).await?;
        while let Some(msg_row) = msg_rows.next().await? {
            let ts: i32 = msg_row.get(0)?;
            let text: String = msg_row.get(1)?;
            let dt = DateTime::<Utc>::from_timestamp(ts as i64, 0);
            let datetime_str = dt.format("%Y-%m-%d %H:%M:%S").to_string();
            let snippet = if text.len() > 50 { &text[..50] } else { &text };
            println!("   * [URGENT @ {}] {}", datetime_str, snippet);
        }
    }
    println!("\nEnd of report.");
    Ok(())
}

/// Save or update member information for a chat.
pub async fn save_member(conn: &mut Connection, chat_peer: &str, user_id: i64, name: &str, username: Option<&str>, bio: Option<&str>, last_seen: i32) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    let mut tx = conn.transaction().await?;
    tx.execute(
        "INSERT OR REPLACE INTO users (user_id, name, username, bio, last_seen) \n         VALUES (?1, ?2, ?3, ?4, ?5);",
        &[&user_id, &name, &username, &bio, &last_seen],
    ).await?;

    tx.execute(
        "INSERT OR REPLACE INTO chat_members (chat_peer, user_id, joined_at) \n         VALUES (?1, ?2, ?3);",
        &[&chat_peer, &user_id, &(Utc::now().timestamp() as i32)],
    ).await?;
    tx.commit().await?;
    Ok(())
}

/// Get all members for a chat.
pub async fn get_chat_members(conn: &Connection, chat_peer: &str) -> Result<Vec<(i64, String, Option<String>, Option<String>)>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "SELECT u.user_id, u.name, u.username, u.bio \n         FROM users u \n         INNER JOIN chat_members cm ON cm.user_id = u.user_id \n         WHERE cm.chat_peer = ?1 \n         ORDER BY u.name COLLATE NOCASE;"
    ).await?;
    let mut members = Vec::new();
    let mut rows = stmt.query(&[&chat_peer]).await?;
    while let Some(row) = rows.next().await? {
        members.push((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(members)
}

/// Get all chats a user is a member of.
pub async fn get_user_chats(conn: &Connection, user_id: i64) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "SELECT chat_peer \n         FROM chat_members \n         WHERE user_id = ?1"
    ).await?;
    let mut result = Vec::new();
    let mut rows = stmt.query(&[&user_id]).await?;
    while let Some(row) = rows.next().await? {
        result.push(row.get(0)?);
    }
    Ok(result)
} 