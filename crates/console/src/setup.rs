//! First run: the one screen Flow shows before it has anything to show.
//!
//! A fresh install has no models, and until it does the console is seven
//! sections reporting nothing - an Overview with no dictations, a History with
//! no rows, a Models screen whose only useful control is a button. So the
//! window opens on this instead: one page, one job, and the rail does not
//! appear until the job is done.
//!
//! The download itself is `flow install --porcelain`, not a copy of it. The
//! pinned revisions and sha256s live in the daemon's `install.rs` and are the
//! reason an install can be trusted; a second downloader in the window would be
//! a second place for them to be wrong.

use crate::control::{meter, quiet_action};
use crate::theme::{ERR, FAINT, FG, GAP, MUTED, PANE_INSET};
use crate::Message;
use iced::widget::{column, container, row, text, Space};
use iced::{Element, Fill, Font, Length};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

/// Whether this machine still needs setting up.
///
/// The speech model alone decides it. Refining is optional by design - the
/// daemon degrades to the raw transcript without it - so a install that
/// deliberately skipped it must not be dragged back through setup every time
/// the window opens.
pub fn needed() -> bool {
    !flow_paths::speech_model_dir().is_dir()
}

/// One line of `flow install --porcelain`, or the end of it.
#[derive(Debug, Clone)]
pub enum Event {
    /// Total bytes this install will fetch.
    Total(u64),
    /// One model and its share of that total, in fetch order.
    Group(String, u64),
    /// Hashing, either checking what is on disk or verifying what arrived.
    Verifying(String),
    /// Now downloading this file.
    Fetching(String),
    /// Bytes done across the whole install.
    Progress(u64),
    /// A file landed. Carries no name because nothing needs one: the bar is
    /// driven by bytes, and the caption by whatever is being worked on next.
    Installed,
    Finished,
    Failed(String),
}

/// A running install, and the handle that can stop it.
///
/// The child is shared rather than owned by the reader thread because skipping
/// refining means killing curl's parent from the UI thread while that reader is
/// still blocked on a line.
#[derive(Clone, Default)]
pub struct Handle(Arc<Mutex<Option<Child>>>);

impl Handle {
    /// Stop the install where it stands. Whatever has already been verified and
    /// renamed into place stays there, and the part file survives for `-C -` to
    /// resume from, so this is "enough for now" rather than "undo".
    pub fn stop(&self) {
        if let Ok(mut held) = self.0.lock() {
            if let Some(child) = held.as_mut() {
                let _ = child.kill();
            }
        }
    }
}

/// Run the installer and stream what it says.
///
/// Same shape as `daemon::stream`: a blocking read on its own thread feeding an
/// unbounded channel. The protocol is one short line per change and the reader
/// has nothing else to do, so a thread is both less code and easier to follow
/// than making this cooperate with the GUI executor.
pub fn install(speech_only: bool) -> (impl iced::futures::Stream<Item = Event>, Handle) {
    let (tx, rx) = iced::futures::channel::mpsc::unbounded();
    let handle = Handle::default();
    let held = handle.0.clone();

    std::thread::spawn(move || {
        let mut command = Command::new("flow");
        command.arg("install").arg("--porcelain");
        if speech_only {
            command.arg("--speech-only");
        }

        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // The installer's own errors, and curl's. Kept off the protocol
            // stream but not thrown away - a failed install without a reason is
            // the worst version of this screen.
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                let _ = tx.unbounded_send(Event::Failed(format!(
                    "flow install did not start: {err}. Is `flow` on your PATH?"
                )));
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        if let Ok(mut slot) = held.lock() {
            *slot = Some(child);
        }

        if let Some(stdout) = stdout {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let Some(event) = parse(&line) else { continue };
                if tx.unbounded_send(event).is_err() {
                    return; // the window closed
                }
            }
        }

        // The reader ended, so the child has closed its stdout - either it
        // finished, it failed, or it was killed for a skip. Only the exit
        // status says which.
        let status = held.lock().ok().and_then(|mut slot| slot.as_mut().map(|c| c.wait()));
        let ok = matches!(status, Some(Ok(status)) if status.success());

        if !ok {
            let mut reason = String::new();
            if let Some(mut stderr) = stderr {
                use std::io::Read;
                let _ = stderr.read_to_string(&mut reason);
            }
            let reason = reason.trim().lines().next_back().unwrap_or_default().to_string();
            let _ = tx.unbounded_send(Event::Failed(if reason.is_empty() {
                "The download stopped before it finished.".into()
            } else {
                reason
            }));
        }
    });

    (rx, handle)
}

/// One protocol line. An unparseable line is skipped rather than treated as a
/// failure: a newer daemon adding an event must not break an older window.
fn parse(line: &str) -> Option<Event> {
    let (word, rest) = match line.split_once(' ') {
        Some((word, rest)) => (word, rest),
        None => (line, ""),
    };
    Some(match word {
        "total" => Event::Total(rest.parse().ok()?),
        "group" => {
            let (label, bytes) = rest.split_once(' ')?;
            Event::Group(label.to_string(), bytes.parse().ok()?)
        }
        "progress" => Event::Progress(rest.parse().ok()?),
        "verifying" => Event::Verifying(rest.to_string()),
        "fetching" => Event::Fetching(rest.split_once(' ').map_or(rest, |(dest, _)| dest).into()),
        "installed" => Event::Installed,
        "finished" => Event::Finished,
        _ => return None,
    })
}

/// Where refining will run, straight from the daemon binary.
///
/// The window cannot work this out itself: enumerating GPUs means llama.cpp,
/// and keeping that tree out of here is the whole reason the console is a
/// second binary. `flow probe` already knows, so it is asked.
///
/// Returns the device's name, or `None` when the daemon binary is not on the
/// PATH - in which case the screen simply says nothing about hardware, which
/// is better than an apology in its place.
///
/// `flow probe` also prints a `detail` line with free memory on it. That is for
/// someone at a terminal asking why; it is deliberately not read here.
pub fn probe() -> Option<String> {
    let output = Command::new("flow").arg("probe").stdin(Stdio::null()).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();

    let value = |key: &str| {
        text.lines()
            .find_map(|line| line.split_once('\t').filter(|(k, _)| *k == key))
            .map(|(_, v)| v.trim().to_string())
    };
    value("refine")
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// How far the install has got, and what it is doing right now.
pub struct State {
    pub total: u64,
    pub done: u64,
    /// The models being fetched and their sizes, in fetch order. Empty until
    /// the installer says; one bar is drawn per entry.
    pub groups: Vec<(String, u64)>,
    /// Where the bar is actually drawn, which chases `done` rather than
    /// matching it. Progress arrives in ~120ms steps of tens of megabytes; a
    /// bar that jumped between them would tick like a clock instead of filling.
    pub shown: f32,
    pub phase: Phase,
    /// The GPU refining will run on, once `flow probe` has answered.
    pub hardware: Option<String>,
    /// Live only while the refining model is the thing downloading.
    pub handle: Handle,
    /// True once the user has asked to stop after the speech model.
    pub skipped: bool,
    /// Reached deliberately from About rather than by having nothing installed.
    /// Changes only what the screen says and whether finishing starts the
    /// daemon - the work it does is identical either way.
    pub rerun: bool,
}

/// One model's slice of the progress bar.
///
/// Carries no label: the caption under the bar already names whichever model
/// is being worked on, and repeating it on the bar would be the same fact
/// twice in one glance.
pub struct Segment {
    /// How much of the bar's width this model is worth, 0 to 1. Proportional
    /// rather than equal halves: the refining model is nearly four times the
    /// speech model, and two equal bars would imply the first was half the
    /// wait when it is closer to a fifth.
    pub share: f32,
    pub filled: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Between the window opening and the first line arriving.
    Starting,
    Verifying(String),
    Fetching(String),
    Done,
    Failed(String),
}

impl Default for State {
    fn default() -> Self {
        Self {
            total: 0,
            done: 0,
            groups: Vec::new(),
            shown: 0.0,
            phase: Phase::Starting,
            hardware: None,
            handle: Handle::default(),
            skipped: false,
            rerun: false,
        }
    }
}

/// How quickly the drawn bar closes the gap to the real one, as a time
/// constant. A quarter-second reads as the bar keeping up with the download
/// rather than trailing it.
const CHASE: f32 = 0.25;

impl State {
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Total(total) => self.total = total,
            // Re-running setup replans from scratch, so the old split must not
            // survive into the new one.
            Event::Group(label, bytes) => self.groups.push((label, bytes)),
            Event::Progress(done) => self.done = self.done.max(done),
            Event::Verifying(what) => self.phase = Phase::Verifying(what),
            Event::Fetching(what) => self.phase = Phase::Fetching(what),
            // The bar is driven by bytes, so a file landing needs no state of
            // its own - but the last file landing is what makes the total true.
            Event::Installed => {}
            Event::Finished => {
                self.done = self.total;
                self.phase = Phase::Done;
            }
            // A kill we asked for is a skip, not a failure. Everything the
            // speech model needed is already verified and on disk.
            //
            // The total comes down to what actually arrived rather than the
            // bar being pushed up to meet it: skipping 2.4 GB and then being
            // told "3.2 GB of 3.2 GB" is the window lying about the thing the
            // user just decided not to do.
            Event::Failed(_) if self.skipped => {
                self.total = self.done;
                self.phase = Phase::Done;
            }
            Event::Failed(why) => self.phase = Phase::Failed(why),
        }
    }

    /// Move the drawn bar toward the real one. Frame-rate independent, so it
    /// settles at the same speed whatever the compositor is doing.
    pub fn advance(&mut self, seconds: f32) {
        let target = self.fraction();
        self.shown += (target - self.shown) * (1.0 - (-seconds / CHASE).exp());
    }

    /// One entry per model: its label, its share of the bar's width, and how
    /// far along it is.
    ///
    /// Derived from the single running byte count rather than tracked per file,
    /// which works because the installer always fetches the models in the order
    /// it announced them: everything below the boundary belongs to the first,
    /// everything above to the second. One eased number still drives both, so
    /// the two bars cannot drift apart or animate at different speeds.
    pub fn segments(&self) -> Vec<Segment> {
        if self.total == 0 || self.groups.is_empty() {
            return Vec::new();
        }

        let shown_bytes = self.shown * self.total as f32;
        let mut offset = 0.0;

        self.groups
            .iter()
            .map(|(_label, bytes)| {
                let bytes = *bytes as f32;
                let filled = ((shown_bytes - offset) / bytes).clamp(0.0, 1.0);
                offset += bytes;
                Segment { share: bytes / self.total as f32, filled }
            })
            .collect()
    }

    fn fraction(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.done as f32 / self.total as f32).clamp(0.0, 1.0)
    }

    /// True while anything is still moving, so the window only asks for frames
    /// when there is something to draw.
    pub fn running(&self) -> bool {
        !matches!(self.phase, Phase::Failed(_)) && (self.shown - self.fraction()).abs() > 0.0005
    }

    /// Whether the optional half is what is downloading. Only then is there
    /// something to skip: stopping during the speech model would leave a Flow
    /// that cannot dictate at all.
    pub fn skippable(&self) -> bool {
        !self.skipped
            && matches!(&self.phase, Phase::Fetching(what) | Phase::Verifying(what)
                if !what.starts_with("tdt/"))
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

/// How wide the column gets, however wide the window is. Setup is a handful of
/// short lines and one bar; stretched to 1040px they would sit as four islands
/// with a lake between them.
const COLUMN: f32 = 460.0;

/// The heading. Named for what the screen is doing, which is not the same job
/// on a first run as on a repair.
pub fn title(state: &State) -> &'static str {
    match (state.phase == Phase::Done, state.rerun) {
        (true, true) => "Everything checks out",
        (true, false) => "Flow is ready",
        (false, true) => "Checking your models",
        (false, false) => "Setting up Flow",
    }
}

/// The line under the heading.
pub fn blurb(state: &State) -> &'static str {
    match (state.phase == Phase::Done, state.rerun, state.skipped) {
        (true, true, _) => "Both models are present and match their published hashes.",
        // Says where to go back for the half that was skipped, and that going
        // back is cheap: the part-downloaded file is kept, so whatever arrived
        // before the skip is not paid for twice. Both halves of that matter at
        // the one moment the decision is still fresh enough to change.
        (true, false, true) => {
            "Speech recognition is installed and the daemon is running. Models can add refining later, picking up where this left off."
        }
        // What just happened, not how to use it. A finished install is a
        // report: the chord is on the next screen, in the Dictation section,
        // and repeating it here made the end of setup read like a tutorial
        // nobody asked for.
        (true, false, false) => "Both models are installed and the daemon is running.",
        // A rerun is repair, and saying "downloaded once" to someone who has
        // already done it would read as the screen having forgotten.
        (false, true, _) => "Each file is checked against its hash, and anything missing is fetched.",
        (false, false, _) => {
            "Two models, downloaded once. Nothing you say will ever leave this machine."
        }
    }
}

pub fn view(state: &State) -> Element<'_, Message> {
    let body = column![
        text(title(state)).size(26).color(FG),
        Space::new().height(10),
        text(blurb(state)).size(13).color(MUTED),
        Space::new().height(28),
        progress_block(state),
        Space::new().height(GAP * 2.0),
        actions(state),
    ]
    .width(Length::Fixed(COLUMN));

    // Three bands: air, the work, air, and then the way out along the bottom
    // edge. The skip is deliberately not in the column - sitting under the bar
    // it read as the next step in the sequence, which is the opposite of what
    // it is. Down here it is available without being offered, and the eye
    // finishes on the download rather than on the way to avoid it.
    //
    // Centred horizontally because this is the only screen with no rail beside
    // it, and a column pinned to the left of a 1040px window would read as the
    // page having failed to load the rest of itself.
    container(
        column![
            Space::new().height(Fill),
            container(body).align_x(iced::alignment::Horizontal::Center),
            Space::new().height(Fill),
            bail_out(state),
        ]
        .align_x(iced::alignment::Horizontal::Center),
    )
    .width(Fill)
    .height(Fill)
    .padding(PANE_INSET)
    .align_x(iced::alignment::Horizontal::Center)
    .into()
}

/// The way past the optional half, along the bottom edge.
///
/// Two lines: what it does, and what it costs. The cost line is the one that
/// matters - "skip" on its own sounds free, and the person reading it is being
/// asked to give up the punctuation that makes dictation worth using.
fn bail_out(state: &State) -> Element<'_, Message> {
    if !state.skippable() {
        return Space::new().height(0).into();
    }

    column![
        quiet_action("Skip refining model", Message::SkipRefine),
        Space::new().height(2),
        text("Flow still works without it — your words arrive raw, with no punctuation and the ums left in.")
            .size(11)
            .color(FAINT)
            .align_x(iced::alignment::Horizontal::Center),
    ]
    .align_x(iced::alignment::Horizontal::Center)
    .width(Length::Fixed(COLUMN))
    .into()
}

/// The bar, and the line under it that says what it is counting.
fn progress_block(state: &State) -> Element<'_, Message> {
    let (left, right) = match &state.phase {
        Phase::Failed(why) => (why.clone(), String::new()),
        _ => (
            activity(state),
            if state.total == 0 {
                String::new()
            } else {
                format!("{} of {}", bytes(state.done), bytes(state.total))
            },
        ),
    };

    let failed = matches!(state.phase, Phase::Failed(_));

    column![
        bars(state, failed),
        Space::new().height(12),
        row![
            // The bar's own caption wraps rather than clips: a curl error is a
            // sentence, not a label, and truncating it would hide the half that
            // says what to do about it.
            container(text(left).size(12).color(if failed { ERR } else { MUTED })).width(Fill),
            Space::new().width(16),
            text(right).size(12).font(Font::MONOSPACE).color(FAINT),
        ],
        Space::new().height(6),
        hardware(state),
    ]
    .into()
}

/// The bar, as one length per model.
///
/// Two downloads shown as one bar hid which of them was running and made the
/// speech model - a fifth of the bytes and the only required half - look like
/// the same job as the refining model. Split, the first fills and stops, and
/// the boundary is where the optional half begins.
///
/// Falls back to a single bar whenever the split is not known: a `--speech-only`
/// install has one model, and the first frames arrive before the installer has
/// said what it is fetching.
fn bars(state: &State, failed: bool) -> Element<'_, Message> {
    let segments = state.segments();
    if segments.len() < 2 {
        return meter(state.shown, failed);
    }

    let mut lengths = row![].spacing(SPLIT);
    for segment in segments {
        // Widths in thousandths, so a 21/79 split survives integer portions.
        let portion = (segment.share * 1000.0).round().max(1.0) as u16;
        lengths = lengths.push(
            container(meter(segment.filled, failed)).width(Length::FillPortion(portion)),
        );
    }
    lengths.into()
}

/// The gap between the two lengths. Wide enough to read as two bars, narrow
/// enough that they still read as one measurement.
const SPLIT: f32 = 5.0;

/// One quiet line naming the card Flow picked, or nothing at all.
///
/// This used to be a four-row panel: the GPU, that speech runs on the CPU and
/// why, the microphone, the session. All of it true, none of it the user's
/// problem - a person installing a dictation tool is not shopping for an
/// inference backend, and being told which device holds which model is the
/// product explaining its own engineering. Neither of the tools this screen
/// takes after says any of it.
///
/// What survives is the half that is genuinely reassuring while 3 GB comes
/// down: your machine can do this, and here is the part of it that will. No
/// free-memory figure, no API name, and nothing at all when there is no card
/// worth naming - the CPU path works, so saying so would only invite worry
/// about a choice the user cannot act on anyway.
fn hardware_line(state: &State) -> Option<String> {
    let device = state.hardware.as_deref()?;
    // "Refining will run on your CPU" is an anxiety rather than a reassurance,
    // and there is nothing the reader could do about it: the fallback exists
    // because it is correct, not because something went wrong.
    if device == "CPU" {
        return None;
    }
    Some(format!("Refining will run on your {device}."))
}

fn hardware(state: &State) -> Element<'_, Message> {
    match hardware_line(state) {
        Some(line) => text(line).size(11).color(FAINT).into(),
        None => Space::new().height(0).into(),
    }
}

/// What the installer is doing, in the product's own words rather than a
/// filename. "encoder-model.int8.onnx" is true and tells nobody anything.
fn activity(state: &State) -> String {
    // Reads as the object of "Downloading" and of "Checking", so both need the
    // article: "Downloading refining" is not a sentence.
    let name = |dest: &str| {
        if dest.starts_with("tdt/") {
            "speech recognition"
        } else {
            "the refining model"
        }
    };
    match &state.phase {
        Phase::Starting => "Starting…".into(),
        Phase::Verifying(dest) => format!("Checking {}", name(dest)),
        Phase::Fetching(dest) => format!("Downloading {}", name(dest)),
        // A skipped install has one model, not two, and saying otherwise
        // would be the screen contradicting the button the user just pressed.
        Phase::Done if state.skipped => "Speech recognition is on this machine".into(),
        Phase::Done => "Both models are on this machine".into(),
        Phase::Failed(why) => why.clone(),
    }
}

/// The page's primary control, and only when there is one.
///
/// Nothing to press while it downloads: the install needs no permission it was
/// not already given by being launched. Skipping is not here - it lives at the
/// bottom of the window, where a way out belongs (see `bail_out`).
fn actions(state: &State) -> Element<'_, Message> {
    let button = match &state.phase {
        Phase::Done => Some(crate::control::action_msg(
            if state.rerun { "Back to settings" } else { "Start Flow" },
            true,
            Message::FinishSetup,
        )),
        Phase::Failed(_) => Some(crate::control::action_msg(
            "Try again",
            true,
            Message::BeginSetup,
        )),
        _ => None,
    };

    let note = match &state.phase {
        // Says what the button will not do, which is the half people worry
        // about: a resumable download is a very different thing to lose.
        Phase::Failed(_) => "Nothing that arrived is lost - it picks up where it stopped.",
        _ => "",
    };

    let Some(button) = button else {
        return Space::new().height(0).into();
    };

    let mut stack =
        column![container(button).width(Fill).align_x(iced::alignment::Horizontal::Center)];
    // Only when there is something to say. An empty caption still reserves its
    // line, which under a finished install left the button floating above a
    // gap that looked like a missing element.
    if !note.is_empty() {
        stack = stack.push(Space::new().height(10)).push(
            container(text(note).size(11).color(FAINT))
                .width(Fill)
                .align_x(iced::alignment::Horizontal::Center),
        );
    }
    stack.into()
}

/// Bytes as a person reads them, in the decimal units a download is quoted in.
/// The Models screen counts what is on disk and uses binary units for it; this
/// counts what is coming over a wire, which is the other convention on purpose.
fn bytes(value: u64) -> String {
    match value {
        0..=999_999 => format!("{} KB", value / 1_000),
        1_000_000..=999_999_999 => format!("{} MB", value / 1_000_000),
        _ => format!("{:.1} GB", value as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_lines_become_events() {
        assert!(matches!(parse("total 3149465119"), Some(Event::Total(3_149_465_119))));
        assert!(matches!(parse("progress 12"), Some(Event::Progress(12))));
        assert!(matches!(parse("finished"), Some(Event::Finished)));
        // The dest is the first field; the size after it belongs to the bar's
        // total, not to the label.
        assert!(
            matches!(parse("fetching tdt/vocab.txt 93939"), Some(Event::Fetching(d)) if d == "tdt/vocab.txt")
        );
    }

    /// A line from a newer daemon must be ignored, not fatal.
    #[test]
    fn unknown_lines_are_skipped() {
        assert!(parse("something-new 4").is_none());
        assert!(parse("").is_none());
        assert!(parse("total not-a-number").is_none());
    }

    /// Only the optional half can be skipped. Stopping during the speech model
    /// would leave a Flow that cannot dictate at all.
    #[test]
    fn only_the_refining_model_is_skippable() {
        let speech = State {
            phase: Phase::Fetching("tdt/encoder-model.int8.onnx".into()),
            ..State::default()
        };
        assert!(!speech.skippable());

        let mut refining = State {
            phase: Phase::Fetching("qwen3-4b-instruct-q4km.gguf".into()),
            ..State::default()
        };
        assert!(refining.skippable());

        // Pressed once, so the control goes away rather than offering a second
        // kill of a child that is already dying.
        refining.skipped = true;
        assert!(!refining.skippable());
    }

    /// A kill the user asked for finishes setup; one they did not is a failure
    /// with something to say about it.
    #[test]
    fn a_skip_is_not_a_failure() {
        let mut state = State { total: 100, done: 30, skipped: true, ..State::default() };
        state.apply(Event::Failed("killed".into()));
        assert_eq!(state.phase, Phase::Done);
        // The bar reads full because what was wanted arrived - not because the
        // 70 that was deliberately skipped is being counted as downloaded.
        assert_eq!((state.total, state.done), (30, 30));

        let mut state = State { total: 100, ..State::default() };
        state.apply(Event::Failed("no route to host".into()));
        assert!(matches!(state.phase, Phase::Failed(_)));
    }

    /// The bar approaches the real figure and settles, rather than snapping to
    /// it or overshooting past it.
    #[test]
    fn the_bar_chases_without_overshooting() {
        let mut state = State { total: 100, done: 100, ..State::default() };
        for _ in 0..200 {
            state.advance(1.0 / 60.0);
            assert!(state.shown <= 1.0, "overshot to {}", state.shown);
        }
        assert!(state.shown > 0.99, "never arrived: {}", state.shown);
        assert!(!state.running());
    }

    /// A repair from About must not greet the user as though they had just
    /// installed Flow, nor promise a download it is not going to do.
    #[test]
    fn a_rerun_says_it_is_checking_not_installing() {
        let checking = State { rerun: true, ..State::default() };
        assert_eq!(title(&checking), "Checking your models");
        assert!(blurb(&checking).contains("checked against its hash"));

        let finished = State { rerun: true, phase: Phase::Done, ..State::default() };
        assert_eq!(title(&finished), "Everything checks out");

        // A finished install reports what happened. Teaching the chord here
        // made the end of setup read as a tutorial, and the chord lives on the
        // Dictation screen the user is about to land on anyway.
        let first_run = State { phase: Phase::Done, ..State::default() };
        assert_eq!(title(&first_run), "Flow is ready");
        assert!(blurb(&first_run).contains("installed"));
        assert!(!blurb(&first_run).contains("Super+Shift+D"));
    }

    /// The line is a reassurance about the machine, so it appears only when
    /// there is a card worth naming.
    #[test]
    fn the_hardware_line_names_a_card_or_says_nothing() {
        let named =
            State { hardware: Some("NVIDIA GeForce RTX 3060 Ti".into()), ..State::default() };
        assert_eq!(
            hardware_line(&named).as_deref(),
            Some("Refining will run on your NVIDIA GeForce RTX 3060 Ti.")
        );

        // Not probed yet, and probed to the CPU, are both silence.
        assert_eq!(hardware_line(&State::default()), None);
        assert_eq!(
            hardware_line(&State { hardware: Some("CPU".into()), ..State::default() }),
            None
        );
    }

    #[test]
    fn progress_never_goes_backwards() {
        let mut state = State { total: 100, ..State::default() };
        state.apply(Event::Progress(60));
        // A part file is stat'd while curl writes it, so a short read after a
        // rename could otherwise pull the bar back.
        state.apply(Event::Progress(12));
        assert_eq!(state.done, 60);
    }
}
