use rusqlite::{Connection, params};
use chrono::DateTime;
use chrono::Utc;
use crate::telegram::ChatInfo;
use crate::telegram::MessageInfo;

/// Initialize the SQLite database and create tables if they don't exist.
pub fn init_db(path: &str) -> Result<Connection, Box<dyn std::error::Error>> {
    let conn = Connection::open(path)?;
    // Enable foreign keys (in case we use any)
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         
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

         CREATE TABLE IF NOT EXISTS messages (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            chat_peer  TEXT,
            msg_id     INTEGER,
            date       INTEGER,
            text       TEXT,
            urgent     BOOLEAN DEFAULT 0,
            UNIQUE(chat_peer, msg_id) ON CONFLICT IGNORE,
            FOREIGN KEY(chat_peer) REFERENCES chats(peer_id)
         );

         -- Normalized tables for users and memberships
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

         -- Create indexes for performance
         CREATE INDEX IF NOT EXISTS idx_chat_members_user ON chat_members(user_id);
         CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);"
    )?;
    Ok(conn)
}

/// Insert or update chat info in the database (without touching AI fields).
pub fn save_chat(conn: &mut Connection, chat: &ChatInfo) -> Result<(), Box<dyn std::error::Error>> {
    // Use INSERT ON CONFLICT to update name and access_hash if the chat already exists, but keep existing category/suggested_name.
    conn.execute(
        "INSERT INTO chats (peer_id, type, tg_id, name, access_hash)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(peer_id) DO UPDATE SET 
            name=excluded.name, access_hash=excluded.access_hash;",
        params![
            chat.peer_id,
            match chat.kind {
                crate::telegram::ChatKind::Group => "Group",
                crate::telegram::ChatKind::Channel => "Channel",
            },
            chat.tg_id,
            chat.title,
            chat.access_hash.unwrap_or(0),
        ],
    )?;
    Ok(())
}

/// Save a batch of new messages for a chat and update the chat's last seen message ID.
pub fn save_messages(conn: &mut Connection, chat_peer: &str, messages: &[MessageInfo]) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    if messages.is_empty() {
        return Ok(());  // nothing to do
    }
    let tx = conn.transaction()?;  // use a transaction for bulk insert
    for msg in messages {
        tx.execute(
            "INSERT OR IGNORE INTO messages (chat_peer, msg_id, date, text) 
             VALUES (?1, ?2, ?3, ?4);",
            params![chat_peer, msg.msg_id, msg.date, msg.text],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Update chat analysis results (category, suggested name, duplicate flag) for a given chat.
pub fn update_chat_analysis(conn: &mut Connection, peer_id: &str, category: &str, suggested_name: &str, duplicate: bool) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    conn.execute(
        "UPDATE chats SET category = ?1, suggested_name = ?2, duplicate = ?3 WHERE peer_id = ?4;",
        params![category, suggested_name, duplicate as i32, peer_id],
    )?;
    Ok(())
}

/// Mark specific messages as urgent in the database.
pub fn mark_urgent(conn: &mut Connection, chat_peer: &str, msg_ids: &[i32]) -> Result<(), Box<dyn std::error::Error>> {
    if msg_ids.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    for &mid in msg_ids {
        tx.execute(
            "UPDATE messages SET urgent = 1 WHERE chat_peer = ?1 AND msg_id = ?2;",
            params![chat_peer, mid],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Generate a report of all chats and any urgent messages, printing to stdout.
pub fn print_report(conn: &Connection) -> Result<(), Box<dyn std::error::Error>> {
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

/// Save or update member information for a chat
pub fn save_member(conn: &mut Connection, chat_peer: &str, user_id: i64, name: &str, username: Option<&str>, bio: Option<&str>, last_seen: i32) 
    -> Result<(), Box<dyn std::error::Error>> 
{
    let tx = conn.transaction()?;
    
    // First update/insert the user record
    tx.execute(
        "INSERT OR REPLACE INTO users (user_id, name, username, bio, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5);",
        params![
            user_id,
            name,
            username,
            bio,
            last_seen,
        ],
    )?;

    // Then ensure the chat membership exists
    tx.execute(
        "INSERT OR REPLACE INTO chat_members (chat_peer, user_id, joined_at)
         VALUES (?1, ?2, ?3);",
        params![
            chat_peer,
            user_id,
            chrono::Utc::now().timestamp() as i32,
        ],
    )?;

    tx.commit()?;
    Ok(())
}

/// Get all members for a chat
pub fn get_chat_members(conn: &Connection, chat_peer: &str) -> Result<Vec<(i64, String, Option<String>, Option<String>)>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "SELECT u.user_id, u.name, u.username, u.bio 
         FROM users u
         INNER JOIN chat_members cm ON cm.user_id = u.user_id
         WHERE cm.chat_peer = ?1 
         ORDER BY u.name COLLATE NOCASE;"
    )?;
    
    let members = stmt.query_map([chat_peer], |row| {
        Ok((
            row.get(0)?, // user_id
            row.get(1)?, // name
            row.get(2)?, // username
            row.get(3)?, // bio
        ))
    })?;
    
    let mut result = Vec::new();
    for member in members {
        result.push(member?);
    }
    Ok(result)
}

/// Get all chats a user is a member of
pub fn get_user_chats(conn: &Connection, user_id: i64) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut stmt = conn.prepare(
        "SELECT chat_peer 
         FROM chat_members 
         WHERE user_id = ?1"
    )?;
    
    let chats = stmt.query_map([user_id], |row| row.get(0))?;
    let mut result = Vec::new();
    for chat in chats {
        result.push(chat?);
    }
    Ok(result)
} 