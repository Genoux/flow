//! Reading what the daemon is doing, over the socket it publishes on.
//!
//! The connection is deliberately unreliable-tolerant: the daemon may not be
//! running when the console opens, may be restarted while it is open, and may
//! never appear at all. None of those are errors worth showing a stack trace
//! for - they are just "Flow isn't running", and the reader keeps trying.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

/// How long to wait before trying the socket again after it goes away. Long
/// enough not to spin on a machine where Flow is simply not installed.
const RETRY: Duration = Duration::from_secs(2);

pub fn socket_path() -> PathBuf {
    flow_paths::socket()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// No daemon on the other end of the socket.
    Offline,
    Starting,
    Ready,
    Listening,
    Working,
}

/// Everything the console knows about the daemon. Starts offline and stays
/// that way until a line arrives.
#[derive(Debug, Clone)]
pub struct State {
    pub activity: Activity,
    pub problem: Option<String>,
    pub words: usize,
}

impl Default for State {
    fn default() -> Self {
        Self { activity: Activity::Offline, problem: None, words: 0 }
    }
}

impl State {
    /// Replace this state from one status line. A line we cannot parse is
    /// ignored rather than treated as a disconnect: a newer daemon adding a
    /// field must not blank the console.
    pub fn apply(&mut self, line: &str) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            return;
        };

        self.activity = match value.get("activity").and_then(|a| a.as_str()) {
            Some("starting") => Activity::Starting,
            Some("ready") => Activity::Ready,
            Some("listening") => Activity::Listening,
            Some("working") => Activity::Working,
            _ => return,
        };

        self.problem = value.get("problem").and_then(|p| p.as_str()).map(str::to_owned);

        self.words = value.get("words").and_then(|w| w.as_u64()).unwrap_or(0) as usize;
    }
}

/// What the reader thread sends up to the application.
#[derive(Debug, Clone)]
pub enum Event {
    Line(String),
    Disconnected,
}

/// Connects, streams every status line, and reconnects forever.
///
/// Blocking socket reads on their own thread rather than async: the protocol is
/// one short line per change, and a thread doing a blocking `read_line` is both
/// less code and easier to reason about than making this cooperate with the
/// GUI executor. The channel is unbounded because dropping a status update is
/// worse than it sounds - lose the last one and the window sits on a stale
/// state forever.
pub fn stream() -> impl iced::futures::Stream<Item = Event> {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();

    std::thread::spawn(move || loop {
        match UnixStream::connect(socket_path()) {
            Ok(socket) => {
                let reader = BufReader::new(socket);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if tx.unbounded_send(Event::Line(line)).is_err() {
                        return; // the window closed
                    }
                }
            }
            Err(_) => {
                // Not running, or not installed. Neither is exceptional.
            }
        }
        if tx.unbounded_send(Event::Disconnected).is_err() {
            return;
        }
        std::thread::sleep(RETRY);
    });

    rx
}
