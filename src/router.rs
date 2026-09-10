//! OpenRouter, which is where every refinement goes. Speech-to-text is local
//! now (see `stt.rs`); this module is the cloud editor and nothing else.
//!
//! curl rather than an HTTP crate, the same call `update.rs` already makes for
//! releases. The request here is one round trip with a JSON reply - no
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

/// Every transport failure - curl missing, the connection refused, the
/// deadline hit - is worded to start with this, and nothing else is. `probe`
/// matches on it to tell a dead network apart from a key OpenRouter answered
/// and refused; a parallel error type for one boolean would outweigh the one
/// string this module already controls end to end.
const UNREACHABLE_PREFIX: &str = "could not reach OpenRouter";

/// Whether an error from [`chat`] means OpenRouter never answered, as
/// opposed to a reply that arrived and said no.
pub fn is_transport_failure(err: &anyhow::Error) -> bool {
    err.to_string().starts_with(UNREACHABLE_PREFIX)
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
            "{UNREACHABLE_PREFIX}{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        );
    }
    serde_json::from_slice(&output.stdout).context("OpenRouter sent something that is not JSON")
}

/// Whether OpenRouter accepts this key, without buying anything.
///
/// `GET /key` is not an inference endpoint, so this costs nothing and can run
/// every time the window opens - which a refining request could not, and that
/// is the whole reason this exists rather than reusing `chat`. Decided on the
/// status line alone: the body's shape is OpenRouter's to change, and 200
/// versus 401 is the entire question.
///
/// `Ok(false)` is a live answer that the key is bad. `Err` means no answer at
/// all, which is a different thing to tell the user.
pub fn key_accepted(key: &str, timeout: Duration) -> Result<bool> {
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        bail!("Invalid OpenRouter key");
    }
    let mut child = Command::new("curl")
        .args(["--silent", "--output", "/dev/null", "-K", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("curl is not installed")?;

    // Same `-K -` handling as `send`: the key never reaches argv.
    let options = format!(
        "url = \"{API}/key\"\n\
         header = \"Authorization: Bearer {key}\"\n\
         write-out = \"%{{http_code}}\"\n\
         max-time = {}\n",
        timeout.as_secs_f64().max(0.001),
    );
    child
        .stdin
        .take()
        .expect("piped")
        .write_all(options.as_bytes())
        .context("handing curl its options")?;

    let output = child.wait_with_output().context("running curl")?;
    let status = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match status.parse::<u16>() {
        Ok(code) if (200..300).contains(&code) => Ok(true),
        Ok(401..=403) => Ok(false),
        Ok(code) => bail!("could not reach OpenRouter: it answered {code}"),
        Err(_) => {
            let reason = String::from_utf8_lossy(&output.stderr);
            let reason = reason.trim();
            bail!(
                "could not reach OpenRouter{}",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(": {reason}")
                }
            )
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_error_is_reported_not_swallowed() {
        let reply = serde_json::json!({"error": {"message": "insufficient credits"}});
        let err = parse_chat(&reply).unwrap_err().to_string();
        assert!(err.contains("insufficient credits"), "{err}");
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

    #[test]
    fn only_a_transport_failure_reads_as_unreachable() {
        let network = anyhow::anyhow!("could not reach OpenRouter: timed out");
        assert!(is_transport_failure(&network));

        let refused = anyhow::anyhow!("OpenRouter refused the text: Missing Authentication header");
        assert!(!is_transport_failure(&refused));
    }
}
