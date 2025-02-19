use ollama_rs::{Ollama, generation::completion::request::GenerationRequest};
use crate::telegram::MessageInfo;
use log::debug;

/// How many recent messages to include in AI prompt (for context).
const CONTEXT_MSG_COUNT: usize = 1000;
/// Default model to use for Ollama (can be overridden via OLLAMA_MODEL env var).
const DEFAULT_MODEL: &str = "mistral-small:latest";

/// Analyze a chat's messages using a local LLM via Ollama.
/// Returns (category, suggested_name, urgent_msg_ids).
pub async fn analyze_chat(chat_title: &str, messages: &[MessageInfo]) -> Result<(String, String, Vec<i32>), Box<dyn std::error::Error>> {
    // Determine which model to use
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
    let ollama = Ollama::default(); // connect to local Ollama at default http://localhost:11434
    // Prepare the prompt with up to CONTEXT_MSG_COUNT latest messages
    let count = messages.len().min(CONTEXT_MSG_COUNT);
    let start_index = messages.len().saturating_sub(CONTEXT_MSG_COUNT);
    // Enumerate messages for clarity in prompt
    let mut prompt_text = format!("The following are recent messages in the chat \"{}\":\n", chat_title);
    for (i, msg) in messages.iter().skip(start_index).enumerate() {
        let idx = i + 1;  // 1-indexed in the prompt
        let text = msg.text.replace('\n', " "); // flatten newlines
        prompt_text += &format!("{}. {}\n", idx, text);
    }
    // Append instructions for the AI
    prompt_text += "\nDetermine a short category for the topic of this chat.\n";
    prompt_text += "Identify if any of the above messages are urgent (needing immediate attention) and list their numbers (or 'None').\n";
    prompt_text += "Suggest a concise, standardized name for this chat.\n";
    prompt_text += "Provide the answer in the format:\n";
    prompt_text += "Category: <category>\nUrgent: <list of message numbers or None>\nSuggestedName: <suggested name>\n";
    debug!("AI Prompt:\n{}", prompt_text);

    // Send the prompt to the local model
    let request = GenerationRequest::new(model, prompt_text);
    let response = ollama.generate(request).await;
    if let Err(e) = response {
        return Err(format!("AI generation failed: {}", e).into());
    }
    let result = response.unwrap();
    let ai_text = result.response.trim().to_string();
    debug!("AI Raw Response:\n{}", ai_text);

    // Parse the model's response to extract insights
    let mut category = String::new();
    let mut suggested_name = String::new();
    let mut urgent_list: Vec<i32> = Vec::new();
    for line in ai_text.lines() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("category:") {
            // e.g. "Category: Project Discussion"
            category = trimmed[9..].trim().to_string();
        } else if trimmed.to_lowercase().starts_with("suggestedname:") {
            // e.g. "SuggestedName: Project Alpha Team"
            suggested_name = trimmed["SuggestedName:".len()..].trim().to_string();
        } else if trimmed.to_lowercase().starts_with("urgent:") {
            // e.g. "Urgent: 3, 4" or "Urgent: None"
            let rest = trimmed[7..].trim();
            if rest.to_lowercase() == "none" || rest.to_lowercase() == "no" {
                urgent_list.clear();
            } else {
                // Extract numbers from the rest of the line
                for token in rest.split(|c| c == ',' || c == ';' || c == ' ') {
                    if let Ok(num) = token.trim().parse::<usize>() {
                        // Map the prompt message index back to actual message ID
                        // Prompt enumerated last `count` messages starting at index 1.
                        // If num is <= count, find that message from the slice we sent.
                        if num >= 1 && num <= count {
                            // We enumerated messages.skip(start_index), so offset index
                            let actual_index = start_index + (num - 1);
                            if let Some(msg) = messages.get(actual_index) {
                                urgent_list.push(msg.msg_id);
                            }
                        }
                    }
                }
            }
        }
    }
    if category.is_empty() {
        category = "Uncategorized".into();
    }
    if suggested_name.is_empty() {
        suggested_name = chat_title.to_string();
    }
    Ok((category, suggested_name, urgent_list))
} 