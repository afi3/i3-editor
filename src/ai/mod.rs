//! Minimal OpenAI-compatible chat client.
//!
//! Network I/O runs through `curl`, matching the updater/installer policy and
//! avoiding a second TLS stack. Requests are non-streaming for now.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use serde_json::{Value, json};

use crate::config::AiConfig;

pub struct Task {
    rx: Receiver<Result<String, String>>,
}

impl Task {
    pub fn try_recv(&self) -> Result<Result<String, String>, mpsc::TryRecvError> {
        self.rx.try_recv()
    }
}

pub fn spawn(config: AiConfig, prompt: String) -> Task {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = ask(&config, &prompt);
        let _ = tx.send(result);
    });
    Task { rx }
}

pub fn ask(config: &AiConfig, prompt: &str) -> Result<String, String> {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return Err("AI prompt is empty".to_string());
    }
    if config.base_url.trim().is_empty() || config.model.trim().is_empty() {
        return Err("AI base_url and model must be configured".to_string());
    }
    let url = completions_url(&config.base_url);
    let body = json!({
        "model": config.model,
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0.2
    });
    let body_text = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    let mut body_file = tempfile::NamedTempFile::new().map_err(|error| error.to_string())?;
    body_file
        .write_all(&body_text)
        .map_err(|error| error.to_string())?;
    let key = std::env::var(&config.api_key_env).unwrap_or_default();
    let mut curl_config = String::new();
    curl_config.push_str(&format!("url = {}\n", quote(&url)));
    curl_config.push_str("request = \"POST\"\n");
    curl_config.push_str("header = \"Content-Type: application/json\"\n");
    if !key.is_empty() {
        curl_config.push_str(&format!(
            "header = {}\n",
            quote(&format!("Authorization: Bearer {key}"))
        ));
    }
    curl_config.push_str(&format!(
        "data-binary = {}\n",
        quote(&format!("@{}", body_file.path().to_string_lossy()))
    ));
    curl_config.push_str(&format!("max-time = {}\n", config.timeout_secs.max(1)));

    let mut child = Command::new("curl")
        .args(["--config", "-", "--silent", "--show-error", "--fail"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("curl: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(curl_config.as_bytes())
            .map_err(|error| format!("curl config: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("curl: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("AI request failed: {}", stderr.trim()));
    }
    let value: Value =
        serde_json::from_slice(&output.stdout).map_err(|error| format!("AI response: {error}"))?;
    value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "AI response has no message content".to_string())
}

fn completions_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn quote(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn asks_an_openai_compatible_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request).expect("read");
            let body = r#"{"choices":[{"message":{"content":"hello from mock"}}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write");
        });
        let config = AiConfig {
            base_url: format!("http://{address}/v1"),
            model: "mock-model".to_string(),
            api_key_env: "I3_EDITOR_TEST_AI_KEY".to_string(),
            timeout_secs: 5,
        };
        let reply = ask(&config, "hello").expect("reply");
        assert_eq!(reply, "hello from mock");
        server.join().expect("server");
    }

    #[test]
    fn rejects_an_empty_prompt() {
        let config = AiConfig::default();
        assert!(ask(&config, "  ").is_err());
    }
}
