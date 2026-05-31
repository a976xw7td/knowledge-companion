//! OpenAI-compatible LLM provider.
//!
//! Calls remote LLM APIs for RAG question answering and translation.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Configuration for an LLM provider.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_seconds: u64,
}

/// A chat message.
#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChatChoiceMessage {
    content: String,
}

/// Call the LLM for RAG question answering.
pub async fn ask(
    config: &LlmConfig,
    system_prompt: &str,
    question: &str,
    context: &str,
) -> Result<String> {
    let full_prompt = format!(
        "{}\n\n## Context from knowledge base\n\n{}\n\n## Question\n\n{}",
        system_prompt, context, question
    );

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: "You are a helpful knowledge assistant. Answer questions based only on the provided context. Cite facts using the source IDs shown in brackets (e.g., [S-abc123]) that appear at the start of each context block. If the context does not contain enough information, say so. Never fabricate citations.".to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: full_prompt,
            },
        ],
        temperature: 0.3,
        max_tokens: 2048,
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.timeout_seconds))
        .build()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("LLM API error ({}): {}", status, body));
    }

    let chat_response: ChatResponse = response.json().await?;
    let answer = chat_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_else(|| "No response from LLM".to_string());

    Ok(answer)
}

/// Synchronous wrapper for ask — uses blocking reqwest client.
/// Called from MCP tools (which are sync) to avoid async runtime requirement.
pub fn ask_sync(config: &LlmConfig, question: &str, context: &str) -> Result<String> {
    let system_prompt = "You are a helpful knowledge assistant. Answer questions based only on the provided context. Cite facts using the source IDs shown in brackets (e.g., [S-abc123]) that appear at the start of each context block. If the context does not contain enough information, say so. Never fabricate citations.";

    let full_prompt = format!(
        "{}\n\n## Context from knowledge base\n\n{}\n\n## Question\n\n{}",
        system_prompt, context, question
    );

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            ChatMessage { role: "system".to_string(), content: "You are a helpful knowledge assistant. Answer questions based only on the provided context. When citing facts, use [S1], [S2] notation to reference sources. If the context does not contain enough information, say so.".to_string() },
            ChatMessage { role: "user".to_string(), content: full_prompt },
        ],
        temperature: 0.3,
        max_tokens: 2048,
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(config.timeout_seconds))
        .build()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow::anyhow!("LLM API error ({}): {}", status, body));
    }

    let chat_response: ChatResponse = response.json()?;
    Ok(chat_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_default())
}

/// Synchronous wrapper for translate.
pub fn translate_sync(
    config: &LlmConfig,
    source_lang: &str,
    target_lang: &str,
    text: &str,
    glossary_terms: &[(String, String)],
) -> Result<String> {
    let glossary_hint = if glossary_terms.is_empty() {
        String::new()
    } else {
        let terms: Vec<String> = glossary_terms
            .iter()
            .map(|(s, t)| format!("- {} → {}", s, t))
            .collect();
        format!(
            "\n\nUse these preferred translations:\n{}",
            terms.join("\n")
        )
    };

    let prompt = format!(
        "Translate the following text from {} to {}. Maintain the original meaning and tone.{}",
        source_lang, target_lang, glossary_hint
    );

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: prompt,
            },
            ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
            },
        ],
        temperature: 0.1,
        max_tokens: 4096,
    };

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(config.timeout_seconds))
        .build()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(anyhow::anyhow!("LLM API error ({}): {}", status, body));
    }

    let chat_response: ChatResponse = response.json()?;
    Ok(chat_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_else(|| text.to_string()))
}

/// Call the LLM for translation (async).
pub async fn translate(
    config: &LlmConfig,
    source_lang: &str,
    target_lang: &str,
    text: &str,
    glossary_terms: &[(String, String)],
) -> Result<String> {
    let glossary_hint = if glossary_terms.is_empty() {
        String::new()
    } else {
        let terms: Vec<String> = glossary_terms
            .iter()
            .map(|(s, t)| format!("- {} → {}", s, t))
            .collect();
        format!(
            "\n\nUse these preferred translations:\n{}",
            terms.join("\n")
        )
    };

    let prompt = format!(
        "Translate the following text from {} to {}. Maintain the original meaning and tone.{}",
        source_lang, target_lang, glossary_hint
    );

    let request = ChatRequest {
        model: config.model.clone(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: prompt,
            },
            ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
            },
        ],
        temperature: 0.1,
        max_tokens: 4096,
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(config.timeout_seconds))
        .build()?;

    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("LLM API error ({}): {}", status, body));
    }

    let chat_response: ChatResponse = response.json().await?;
    let answer = chat_response
        .choices
        .first()
        .map(|c| c.message.content.clone())
        .unwrap_or_else(|| text.to_string());

    Ok(answer)
}
