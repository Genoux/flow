//! OpenRouter, which is where every transcription and every refinement now
//! goes.
//!
//! curl rather than an HTTP crate, the same call `update.rs` already makes for
//! releases. Both requests here are one round trip with a JSON reply - no
//! streaming, no connection reuse worth keeping - and pulling reqwest, rustls
//! and an async runtime into a synchronous daemon buys none of that back.
//!
//! The key never reaches curl's argv. `/proc/<pid>/cmdline` is readable by
//! other local users on a default kernel, and a billable credential sitting
//! there for the length of every dictation is not worth the shorter function:
//! options go in on stdin via `-K -`, and the request body goes in a 0600
//! temporary file that only its path is passed by.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const API: &str = "https://openrouter.ai/api/v1";

/// The editor used by both release channels, pinned to the provider it was
/// measured against. `allow_fallbacks: false` is deliberate: a silent reroute to another
/// host serving the same model id is a different machine with different
/// arithmetic, and the prompt was tuned against this one.
pub const REFINE_MODEL: &str = "google/gemini-3.1-flash-lite";
const REFINE_PROVIDER: &str = "google-ai-studio";

/// The recognizer the experiment settled on.
pub const STT_MODEL: &str = "microsoft/mai-transcribe-2";

/// Ask the editor to clean one dictation.
pub fn chat(key: &str, system: &str, user: &str, timeout: Duration) -> Result<String> {
    let body = serde_json::json!({
        "model": REFINE_MODEL,
        "temperature": 0,
        "max_tokens": 2048,
        "reasoning": {"effort": "minimal"},
        "provider": {"only": [REFINE_PROVIDER], "allow_fallbacks": false},
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
    });
    parse_chat(&post(key, "/chat/completions", &body.to_string(), timeout)?)
}

/// Transcribe one utterance. `wav` is a complete RIFF file, not raw samples.
pub fn transcribe(key: &str, wav: &[u8], timeout: Duration) -> Result<String> {
    let body = serde_json::json!({
        "model": STT_MODEL,
        "input_audio": {"data": base64(wav), "format": "wav"},
    });
    parse_transcript(&post(
        key,
        "/audio/transcriptions",
        &body.to_string(),
        timeout,
    )?)
}

/// The transcript, or why there isn't one.
fn parse_transcript(reply: &serde_json::Value) -> Result<String> {
    if let Some(message) = error_message(reply) {
        bail!("OpenRouter refused the audio: {message}");
    }
    let Some(text) = reply.get("text").and_then(|t| t.as_str()) else {
        bail!("OpenRouter sent no transcript");
    };
    Ok(text.trim().to_string())
}

/// The edited text, or why there isn't one.
///
/// A truncated reply is a failure, not a shorter dictation: `length` means the
/// model ran out of room mid-sentence, and pasting half a sentence is worse
/// than pasting the raw transcript the caller still holds.
fn parse_chat(reply: &serde_json::Value) -> Result<String> {
    if let Some(message) = error_message(reply) {
        bail!("OpenRouter refused the text: {message}");
    }
    let Some(choice) = reply.get("choices").and_then(|c| c.get(0)) else {
        bail!("OpenRouter sent no choices");
    };
    match choice.get("finish_reason").and_then(|r| r.as_str()) {
        Some("stop") => {}
        Some(reason) => bail!("the editor stopped early ({reason})"),
        None => bail!("the editor did not say why it stopped"),
    }
    let text = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default();
    if text.trim().is_empty() {
        bail!("the editor returned nothing");
    }
    Ok(text.trim().to_string())
}

fn error_message(reply: &serde_json::Value) -> Option<String> {
    let error = reply.get("error")?;
    Some(
        error
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("no reason given")
            .to_string(),
    )
}

/// One POST. Options on stdin, body in a private temporary file, so neither the
/// key nor the dictation is visible in the process list.
fn post(key: &str, path: &str, body: &str, timeout: Duration) -> Result<serde_json::Value> {
    let scratch = scratch_file();
    write_private(&scratch, body.as_bytes()).context("staging the request")?;
    let result = send(key, path, &scratch, timeout);
    let _ = std::fs::remove_file(&scratch);
    result
}

fn send(key: &str, path: &str, scratch: &Path, timeout: Duration) -> Result<serde_json::Value> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        bail!("Invalid OpenRouter key");
    }
    let mut child = Command::new("curl")
        .args(["--silent", "--show-error", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("curl is not installed")?;

    // `-K -` takes one option per line. Values are quoted so a path with a
    // space survives; nothing here is attacker-controlled, and the key is the
    // whole reason this is not on the command line.
    let options = format!(
        "url = \"{API}{path}\"\n\
         request = \"POST\"\n\
         header = \"Content-Type: application/json\"\n\
         header = \"Authorization: Bearer {key}\"\n\
         data-binary = \"@{}\"\n\
         max-time = {}\n\
         fail-with-body\n",
        scratch
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r"),
        timeout.as_secs_f64().max(0.001),
    );
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(options.as_bytes())
        .context("handing curl its options")?;

    let output = child.wait_with_output().context("running curl")?;
    if !output.status.success() && output.stdout.is_empty() {
        // curl's own diagnostics, which never contain the key: it went in on
        // stdin, and curl does not echo its config.
        let reason = String::from_utf8_lossy(&output.stderr);
        let reason = reason.trim();
        bail!(
            "could not reach OpenRouter{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        );
    }
    serde_json::from_slice(&output.stdout).context("OpenRouter sent something that is not JSON")
}

fn scratch_file() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        ".flow-request-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)
}

/// Standard base64, which is what the audio field wants. Small enough to spell
/// out that adding a crate for it would be the larger change.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let block = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(block >> (18 - 6 * i) & 0b11_1111) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_reference_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        // A RIFF header is the real input, and it is the byte range where an
        // off-by-one in the shift arithmetic would show up.
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn a_transcript_is_trimmed() {
        let reply = serde_json::json!({"text": "  hello there\n"});
        assert_eq!(parse_transcript(&reply).unwrap(), "hello there");
    }

    #[test]
    fn a_provider_error_is_reported_not_swallowed() {
        let reply = serde_json::json!({"error": {"message": "insufficient credits"}});
        let err = parse_transcript(&reply).unwrap_err().to_string();
        assert!(err.contains("insufficient credits"), "{err}");

        let err = parse_chat(&reply).unwrap_err().to_string();
        assert!(err.contains("insufficient credits"), "{err}");
    }

    #[test]
    fn a_reply_with_no_transcript_is_an_error() {
        assert!(parse_transcript(&serde_json::json!({"usage": {}})).is_err());
    }

    #[test]
    fn only_a_finished_edit_is_accepted() {
        let done = serde_json::json!({
            "choices": [{"finish_reason": "stop", "message": {"content": " Hello there. "}}]
        });
        assert_eq!(parse_chat(&done).unwrap(), "Hello there.");

        // Truncation is the dangerous one: it returns real-looking text.
        let cut = serde_json::json!({
            "choices": [{"finish_reason": "length", "message": {"content": "Hello the"}}]
        });
        let err = parse_chat(&cut).unwrap_err().to_string();
        assert!(err.contains("stopped early"), "{err}");
    }

    #[test]
    fn an_empty_edit_is_an_error_not_an_empty_paste() {
        let blank = serde_json::json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "   "}}]
        });
        assert!(parse_chat(&blank).is_err());
    }
}
