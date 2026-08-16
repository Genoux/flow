//! What the daemon is doing, published for anything that wants to watch.
//!
//! A Unix socket rather than a status file, because the interesting states are
//! momentary: "listening" begins and ends inside a couple of seconds, and a
//! watcher that polled a file would miss most of them. Clients connect, get the
//! current state immediately, then a line per change.
//!
//! Newline-delimited JSON, built with `serde_json::json!` so no derive is
//! needed on either side.
//!
//! # Nothing here may ever cost a dictation
//!
//! Every socket in this module is non-blocking and every write is best effort.
//! A console that stops reading, or a client that dies mid-write, must not slow
//! the audio path down by a single millisecond - so a would-block is treated
//! exactly like a dead client: the update is dropped, and the recording carries
//! on. This is deliberate. Status is the least important thing the daemon does.

use serde_json::json;
use std::collections::VecDeque;
use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// How many finished dictations the daemon remembers for the console's
/// "Recent" list. Small on purpose: this is a glance, not a history, and the
/// whole state is re-sent to every client that connects.
const RECENT: usize = 8;

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("flow.sock")
}

/// What the daemon is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Starting,
    Ready,
    Listening,
    Working,
}

impl Activity {
    fn name(self) -> &'static str {
        match self {
            Activity::Starting => "starting",
            Activity::Ready => "ready",
            Activity::Listening => "listening",
            Activity::Working => "working",
        }
    }
}

/// One finished dictation, as the console lists it.
#[derive(Debug, Clone)]
pub struct Dictation {
    pub text: String,
    pub spoken: f32,
    pub paste_ms: u128,
}

struct State {
    activity: Activity,
    /// Set when something the user needs to act on has gone wrong - a failed
    /// injection, a missing model. Cleared by the next success.
    problem: Option<String>,
    recent: VecDeque<Dictation>,
    words: usize,
    clients: Vec<UnixStream>,
}

impl State {
    fn snapshot(&self) -> String {
        let recent: Vec<_> = self
            .recent
            .iter()
            .map(|d| {
                json!({
                    "text": d.text,
                    "spoken": d.spoken,
                    "paste_ms": d.paste_ms,
                })
            })
            .collect();

        json!({
            "activity": self.activity.name(),
            "problem": self.problem,
            "words": self.words,
            "recent": recent,
        })
        .to_string()
    }

    /// Write the current state to every client, dropping the ones that have
    /// gone away or stopped reading. See the module note: a slow client is
    /// dropped rather than waited for.
    fn broadcast(&mut self) {
        let line = format!("{}\n", self.snapshot());
        self.clients
            .retain_mut(|client| client.write_all(line.as_bytes()).is_ok());
    }
}

/// Handle the daemon uses to report what it is doing. Cloning is cheap and
/// every clone talks to the same state.
#[derive(Clone)]
pub struct Reporter {
    state: Arc<Mutex<State>>,
}

impl Reporter {
    /// Starts the listener thread. A failure here is reported and swallowed:
    /// the daemon must still dictate on a machine where the socket cannot be
    /// created, exactly as it still dictates without a cleanup model.
    pub fn spawn() -> Self {
        let reporter = Self {
            state: Arc::new(Mutex::new(State {
                activity: Activity::Starting,
                problem: None,
                recent: VecDeque::new(),
                words: 0,
                clients: Vec::new(),
            })),
        };

        let path = socket_path();
        // A socket left by a killed daemon would refuse to bind; nothing else
        // owns this path, so removing it is safe.
        let _ = std::fs::remove_file(&path);

        match UnixListener::bind(&path) {
            Ok(listener) => {
                let state = Arc::clone(&reporter.state);
                std::thread::spawn(move || {
                    for incoming in listener.incoming() {
                        let Ok(client) = incoming else { continue };
                        // Non-blocking before it is ever written to, so a
                        // console that stops reading can never stall a paste.
                        if client.set_nonblocking(true).is_err() {
                            continue;
                        }
                        let mut state = state.lock().expect("status state");
                        let line = format!("{}\n", state.snapshot());
                        let mut client = client;
                        if client.write_all(line.as_bytes()).is_ok() {
                            state.clients.push(client);
                        }
                    }
                });
                eprintln!("status socket: {}", path.display());
            }
            Err(err) => eprintln!("status socket unavailable ({err}); console cannot attach"),
        }

        reporter
    }

    fn set(&self, activity: Activity) {
        let mut state = self.state.lock().expect("status state");
        state.activity = activity;
        state.broadcast();
    }

    pub fn ready(&self) {
        self.set(Activity::Ready);
    }

    pub fn listening(&self) {
        self.set(Activity::Listening);
    }

    pub fn working(&self) {
        self.set(Activity::Working);
    }

    /// Something the user has to fix. Stays up until the next finished
    /// dictation clears it.
    pub fn problem(&self, message: impl Into<String>) {
        let mut state = self.state.lock().expect("status state");
        state.problem = Some(message.into());
        state.broadcast();
    }

    /// A dictation landed. Clears any problem, since the thing evidently works.
    pub fn finished(&self, dictation: Dictation) {
        let mut state = self.state.lock().expect("status state");
        state.words += dictation.text.split_whitespace().count();
        state.recent.push_front(dictation);
        state.recent.truncate(RECENT);
        state.problem = None;
        state.activity = Activity::Ready;
        state.broadcast();
    }
}

pub fn remove_socket() {
    let _ = std::fs::remove_file(socket_path());
}
