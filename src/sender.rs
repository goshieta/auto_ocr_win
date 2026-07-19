use std::error::Error;
use serde::Serialize;
use reqwest::Client;

use crate::config::Config;

/// JSON payload structure for the OCR data.
#[derive(Serialize)]
struct OcrPayload {
    source: String,
    content: String,
}

/// Format a reqwest error into a human-readable diagnostic string,
/// distinguishing between timeout, connection refused, DNS failure, etc.
/// Shows ALL applicable error categories since a single error can match multiple.
fn format_http_error(error: reqwest::Error, url: &str) -> String {
    let mut tags: Vec<&str> = Vec::new();

    if error.is_timeout() {
        tags.push("TIMEOUT");
    }
    if error.is_connect() {
        tags.push("CONNECT");
    }
    if error.is_request() {
        tags.push("REQUEST");
    }
    if error.is_body() {
        tags.push("BODY");
    }
    if error.is_decode() {
        tags.push("DECODE");
    }
    if error.is_redirect() {
        tags.push("REDIRECT");
    }
    if error.is_status() {
        tags.push("STATUS");
    }

    let tags_str = if tags.is_empty() {
        "UNKNOWN".to_string()
    } else {
        tags.join("+")
    };

    // Build the full error chain
    let mut chain = String::new();
    chain.push_str(&format!("{}", error));
    let mut source = error.source();
    while let Some(s) = source {
        chain.push_str(&format!(" \\\\ Caused by: {}", s));
        source = s.source();
    }

    format!(
        "[{}] POST {} failed: {}",
        tags_str, url, chain
    )
}

#[derive(Clone)]
pub struct DataSender {
    client: Client,
    server_url: String,
}

impl DataSender {
    pub fn new(config: &Config) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        DataSender {
            client,
            server_url: config.server_url.clone(),
        }
    }

    /// Build a JSON payload string from the recognized text.
    pub fn build_payload(&self, text: &str) -> Result<String, String> {
        let payload = OcrPayload {
            source: "ocr".to_string(),
            content: text.to_string(),
        };

        serde_json::to_string(&payload)
            .map_err(|e| format!("JSON serialization failed: {}", e))
    }

    /// Send a JSON payload to the configured server URL via HTTP POST.
    pub async fn send(&self, payload: &str) -> Result<(), String> {
        let response = self
            .client
            .post(&self.server_url)
            .header("Content-Type", "application/json")
            .body(payload.to_string())
            .send()
            .await
            .map_err(|e| format_http_error(e, &self.server_url))?;

        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            // Try to read the response body for more context
            let body_text = response
                .text()
                .await
                .unwrap_or_else(|_| "(body read failed)".to_string());
            Err(format!(
                "Server returned {} for POST {}. Response body: {}",
                status.as_u16(),
                &self.server_url,
                &body_text[..body_text.len().min(500)],
            ))
        }
    }
}