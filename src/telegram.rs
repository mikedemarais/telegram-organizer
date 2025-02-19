use grammers_client::{Client, Config, SignInError};
use grammers_client::types::Chat;  // Chat enum (Private, Group, Channel, etc.)
use grammers_client::grammers_tl_types as tl;  // Telegram TL types (for InputPeer and requests)
use std::io::{self, Write, BufRead};
use tokio;

/// Holds minimal info about a chat for our monitoring purposes.
pub struct ChatInfo {
    pub peer_id: String,       // Unique identifier string (e.g. "group:123456")
    pub title: String,         // Chat title
    pub kind: ChatKind,
    pub tg_id: i64,            // Telegram's numeric ID for the chat
    pub access_hash: Option<i64>,  // Access hash for channels/private chats (None for basic groups)
}

/// Enum to distinguish chat type.
#[derive(Debug, Clone)]
pub enum ChatKind { Group, Channel }

/// Holds relevant message data.
pub struct MessageInfo {
    pub msg_id: i32,       // Message ID within the chat
    pub date: i32,         // UNIX timestamp of the message (UTC)
    pub text: String,
}

/// Connect to Telegram and ensure authorization. Saves session to `session_file`.
pub async fn connect(api_id: u32, api_hash: &str, session_file: &str) 
    -> Result<Client, Box<dyn std::error::Error>> 
{
    // Create or load an existing session
    let client = Client::connect(Config {
        session: grammers_client::session::Session::load_file_or_create(session_file)?,
        api_id: api_id.try_into().unwrap(),
        api_hash: api_hash.to_string(),
        params: Default::default(),
    }).await?;

    // If not logged in, perform login flow
    if !client.is_authorized().await? {
        println!("First-time login: please enter your Telegram credentials.");
        let phone = prompt("Enter your phone number (international format): ")?;
        let token = client.request_login_code(&phone.trim()).await?;
        let code = prompt("Enter the login code you received: ")?;
        let sign_in_result = client.sign_in(&token, &code.trim()).await;
        match sign_in_result {
            Err(SignInError::PasswordRequired(password_token)) => {
                // Two-factor authentication (password) is enabled
                let hint = password_token.hint().unwrap_or("none");
                let prompt_msg = format!("Enter your password (hint: {}): ", hint);
                let password = prompt(&prompt_msg)?;
                client.check_password(password_token, password.trim()).await?;
            }
            Err(e) => {
                return Err(format!("Login failed: {}", e).into());
            }
            Ok(_) => {
                // Logged in successfully with code (no password needed)
            }
        }
        println!("Logged in to Telegram successfully.");
        // Save session for future runs
        if let Err(e) = client.session().save_to_file(session_file) {
            eprintln!("Warning: failed to save session file: {}", e);
            eprintln!("You will need to log in again next time.");
        }
    }
    Ok(client)
}

/// Prompt user for input on the console.
fn prompt(message: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stdout = io::stdout();
    write!(stdout, "{}", message)?;
    stdout.flush()?;
    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line)
}

/// Fetch all chat dialogs and return info for group chats (including supergroups/channels).
pub async fn fetch_dialogs(client: &Client) -> Result<Vec<ChatInfo>, Box<dyn std::error::Error>> {
    let mut dialog_iter = client.iter_dialogs();
    let mut chats = Vec::new();
    while let Some(dialog) = dialog_iter.next().await? {
        let chat = dialog.chat();  // grammers_client::types::Chat
        // Filter only group chats and channels (skip private chats)
        let info = match chat {
            Chat::User(_) => {
                continue; // Skip direct user conversations
            }
            Chat::Group(group) => {
                // Basic group chats (legacy groups)
                ChatInfo {
                    peer_id: format!("group:{}", group.id()),
                    title: group.title().to_string(),
                    kind: ChatKind::Group,
                    tg_id: group.id() as i64,
                    access_hash: None,  // not needed for InputPeerChat
                }
            }
            Chat::Channel(channel) => {
                // Channels or supergroups
                ChatInfo {
                    peer_id: format!("channel:{}", channel.id()),
                    title: channel.title().to_string(),
                    kind: ChatKind::Channel,
                    tg_id: channel.id() as i64,
                    access_hash: channel.raw.access_hash,  // Already an Option<i64>
                }
            }
        };
        chats.push(info);
    }
    Ok(chats)
}

/// Fetch new messages for a given chat since the last seen message ID. 
/// Returns a list of new MessageInfo (empty if no new messages).
pub async fn fetch_new_messages(client: &Client, chat: &ChatInfo, last_seen_id: Option<i32>) 
    -> Result<Vec<MessageInfo>, Box<dyn std::error::Error>> 
{
    let mut new_messages = Vec::new();
    // Determine InputPeer for the chat based on type
    let peer = match chat.kind {
        ChatKind::Group => {
            let chat_id = chat.tg_id as i32;
            tl::enums::InputPeer::Chat(tl::types::InputPeerChat { 
                chat_id: chat_id.into() 
            })
        }
        ChatKind::Channel => {
            let channel_id = chat.tg_id as i32;
            let access_hash = chat.access_hash.unwrap_or(0);
            tl::enums::InputPeer::Channel(tl::types::InputPeerChannel { 
                channel_id: channel_id.into(), 
                access_hash 
            })
        }
    };

    // Use last_seen_id or 0 if none (0 will fetch latest messages).
    let last_id = last_seen_id.unwrap_or(0);
    let min_id = if last_id > 0 { last_id } else { 0 };
    let mut max_id = 0;  // 0 means no upper bound
    loop {
        let req = tl::functions::messages::GetHistory {
            peer: peer.clone(),
            offset_id: 0,
            offset_date: 0,
            add_offset: 0,
            limit: 100,
            max_id,
            min_id,
            hash: 0,
        };
        // Invoke the raw Telegram API request
        let result = client.invoke(&req).await;
        if let Err(e) = result {
            // Check for specific error types
            let error_str = e.to_string().to_lowercase();
            if error_str.contains("peer_id_invalid") || 
               error_str.contains("chat_id_invalid") || 
               error_str.contains("channel_invalid") {
                // For invalid peer errors, return empty list (chat will be refreshed next cycle)
                return Ok(Vec::new());
            }
            // For other errors (like flood/rate limits), break this loop
            eprintln!("Error fetching history for {}: {}", chat.title, e);
            break;
        }
        let history = result.unwrap();
        // The result can be of different types; we handle normal messages
        let messages = match history {
            tl::enums::messages::Messages::Messages(messages) => messages.messages,
            tl::enums::messages::Messages::Slice(slice) => slice.messages,
            tl::enums::messages::Messages::ChannelMessages(channel) => channel.messages,
            tl::enums::messages::Messages::NotModified(_) => {
                // NotModified means no new messages (cache hash unchanged)
                Vec::new()
            }
        };
        if messages.is_empty() {
            break;
        }
        // Convert each TL message to MessageInfo
        for msg in &messages {
            if let tl::enums::Message::Message(m) = msg {
                let text = m.message.clone();
                new_messages.push(MessageInfo {
                    msg_id: m.id,
                    date: m.date, 
                    text,
                });
            }
        }
        // If we got fewer than limit, we've fetched all new messages
        if messages.len() < 100 {
            break;
        }
        // Otherwise, prepare to fetch older messages in this range:
        // determine the smallest message ID we got, and use it as new max_id for next call.
        let min_msg_id_in_batch = new_messages.iter().map(|m| m.msg_id).min().unwrap_or(0);
        // Set max_id to that min_msg_id (exclusive) to get older messages in next iteration.
        max_id = min_msg_id_in_batch;
        // Continue loop to fetch more (older) messages above last_seen_id.
    }
    // Sort messages in ascending order by ID (chronological)
    new_messages.sort_by_key(|m| m.msg_id);
    Ok(new_messages)
}

/// Fetch members of a chat and return their information.
pub async fn fetch_chat_members(client: &Client, chat: &ChatInfo) 
    -> Result<Vec<(i64, String, Option<String>, Option<String>)>, Box<dyn std::error::Error>> 
{
    let mut members = Vec::new();
    
    // Different API calls needed for different chat types
    match chat.kind {
        ChatKind::Group => {
            let chat_id = chat.tg_id as i32;
            let req = tl::functions::messages::GetFullChat {
                chat_id: chat_id.into(),
            };
            if let Ok(full_chat) = client.invoke(&req).await {
                let full = match full_chat {
                    tl::enums::messages::ChatFull::Full(f) => f,
                };
                
                if let tl::enums::ChatFull::Full(chat_full) = full.full_chat {
                    // Extract participants
                    if let tl::enums::ChatParticipants::Participants(participants) = chat_full.participants {
                        for participant in participants.participants {
                            if let tl::enums::ChatParticipant::Participant(p) = participant {
                                // Find user in users vector
                                if let Some(user) = full.users.iter().find(|u| {
                                    if let tl::enums::User::User(u) = u {
                                        u.id == p.user_id
                                    } else {
                                        false
                                    }
                                }) {
                                    if let tl::enums::User::User(user) = user {
                                        // Add delay between user info requests to avoid rate limits
                                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                                        
                                        // Get basic user info from the user object
                                        let name = format!("{} {}", 
                                            user.first_name.as_deref().unwrap_or(""),
                                            user.last_name.as_deref().unwrap_or("")).trim().to_string();
                                        
                                        members.push((user.id as i64, name, user.username.clone(), None));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        ChatKind::Channel => {
            let channel_id = chat.tg_id as i32;
            let access_hash = chat.access_hash.unwrap_or(0);
            let req = tl::functions::channels::GetFullChannel {
                channel: tl::types::InputChannel {
                    channel_id: channel_id.into(),
                    access_hash,
                }.into(),
            };
            if let Ok(full_channel) = client.invoke(&req).await {
                let _full = match full_channel {
                    tl::enums::messages::ChatFull::Full(f) => f,
                };
                
                // Get participant list for channel
                let participants_req = tl::functions::channels::GetParticipants {
                    channel: tl::types::InputChannel {
                        channel_id: channel_id.into(),
                        access_hash,
                    }.into(),
                    filter: tl::types::ChannelParticipantsRecent {}.into(),
                    offset: 0,
                    limit: 100,
                    hash: 0,
                };
                
                if let Ok(participants) = client.invoke(&participants_req).await {
                    if let tl::enums::channels::ChannelParticipants::Participants(data) = participants {
                        for user in data.users {
                            if let tl::enums::User::User(user) = user {
                                // Add delay between user info requests
                                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                                
                                // Get basic user info from the user object
                                let name = format!("{} {}", 
                                    user.first_name.as_deref().unwrap_or(""),
                                    user.last_name.as_deref().unwrap_or("")).trim().to_string();
                                
                                members.push((user.id as i64, name, user.username.clone(), None));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(members)
}

/// Helper function to get user information with proper error handling
async fn get_user_info(client: &Client, user_id: i32, access_hash: i64) -> Result<(String, Option<String>, Option<String>), Box<dyn std::error::Error>> {
    let req = tl::functions::users::GetFullUser {
        id: tl::enums::InputUser::User(tl::types::InputUser {
            user_id: user_id.into(),
            access_hash,
        }),
    };
    
    // Try up to 3 times with exponential backoff
    let mut retry_delay = 2;
    for _ in 0..3 {
        match client.invoke(&req).await {
            Ok(full_user) => {
                let full = match full_user {
                    tl::enums::users::UserFull::Full(f) => f,
                };
                
                if let Some(user) = full.users.first() {
                    if let tl::enums::User::User(user) = user {
                        let name = format!("{} {}", 
                            user.first_name.as_deref().unwrap_or(""),
                            user.last_name.as_deref().unwrap_or("")).trim().to_string();
                        
                        // Get bio from full user info
                        let full_user = match full.full_user {
                            tl::enums::UserFull::Full(f) => f,
                        };
                        
                        return Ok((name, user.username.clone(), full_user.about));
                    }
                }
                return Ok((format!("User {}", user_id), None, None));
            }
            Err(e) => {
                let error_str = e.to_string().to_lowercase();
                if error_str.contains("flood") {
                    // If we hit flood wait, sleep and retry with exponential backoff
                    tokio::time::sleep(tokio::time::Duration::from_secs(retry_delay)).await;
                    retry_delay *= 2; // Exponential backoff
                    continue;
                }
                return Err(e.into());
            }
        }
    }
    Ok((format!("User {}", user_id), None, None))
} 