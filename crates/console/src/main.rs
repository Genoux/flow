//! Flow's status and settings window.
//!
//! A separate binary from the daemon on purpose: iced brings wgpu with it, and
//! the daemon has no business carrying that to record audio. The two talk over
//! the status socket the daemon already publishes, and everything else on
//! screen is read from the same files the daemon uses: `settings` edits
//! `config.toml`, `history` reads the transcript log, `vocabulary` edits
//! `vocabulary.txt`.

#[cfg(target_os = "linux")]
mod chord;

/// Capturing a chord means reading `/dev/input` below the compositor, and only
/// Linux has it. Elsewhere the console is a window for looking at and laying
/// out - the picker reports itself unavailable, which is the same answer Linux
/// gives when `/dev/input` is not readable, so no call site changes.
#[cfg(not(target_os = "linux"))]
mod chord {
    pub fn available() -> bool {
        false
    }

    pub fn capture(_cancel: &dyn Fn() -> bool) -> Option<String> {
        None
    }
}

mod calendar;
mod card;
mod control;
mod daemon;
mod demo;
mod format;
mod history;
mod layout;
mod settings;
mod setup;
mod system;
mod theme;
mod update;
mod vocabulary;

use crate::calendar::{calendar_card, current_streak, longest_streak};
use crate::card::{fact, panel, stat_tile};
use crate::control::{
    action_msg, card_rule, hairline, pip, toggle, value_slider, vertical_hairline,
};
use crate::format::{clip, commas, plural, trend};
use crate::layout::{
    entry_list, entry_row, fact_path, fact_row, heading, model_row, nav, path_cta, scroll,
    scroll_inset, section_shell, setting,
};
use crate::theme::{
    mix, progress, ACCENT, BG, CALENDAR_DAYS, CONTENT_RIGHT, COPIED, ENTRY_INSET, ERR, FADE, FAINT,
    FG, GAP, KNOB, LINE, MUTED, OK, PAGE_TOP, PANE_INSET, RAIL_WIDTH, ROW_PAD, SCROLL_PAD,
};
use iced::widget::{column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Fill, Font, Length, Subscription, Task, Theme};

fn main() -> iced::Result {
    iced::application(Console::new, Console::update, Console::view)
        .title("Flow")
        .theme(theme)
        .subscription(subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1040.0, 680.0),
            position: iced::window::Position::Centered,
            min_size: Some(iced::Size::new(640.0, 460.0)),
            // Without this the Wayland app_id is empty, so compositor window
            // rules, taskbars and .desktop matching have nothing to key on.
            // The field is itself Linux-only - macOS names its window through
            // the bundle, and its PlatformSpecific has different fields
            // entirely, so this cannot be set unconditionally.
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: "flow-console".to_string(),
                ..Default::default()
            },
            ..Default::default()
        })
        .style(style)
        .run()
}

// Named rather than closures: the builder needs these to be general over the
// borrow, and an inline closure infers a lifetime that is too specific.
fn theme(_state: &Console) -> Theme {
    Theme::Dark
}

fn style(_state: &Console, _theme: &Theme) -> iced::theme::Style {
    iced::theme::Style { background_color: BG, text_color: FG }
}

fn subscription(state: &Console) -> Subscription<Message> {
    let daemon =
        Subscription::run(|| iced::futures::StreamExt::map(daemon::stream(), Message::Daemon));
    if !state.moving() {
        // Redrawing every frame forever to animate nothing would be a way to
        // make a settings window cost battery.
        return daemon;
    }
    Subscription::batch([daemon, iced::window::frames().map(Message::Tick)])
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    History,
    Dictation,
    Audio,
    Vocabulary,
    Models,
    About,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Overview,
        Section::History,
        Section::Dictation,
        Section::Audio,
        Section::Vocabulary,
        Section::Models,
        Section::About,
    ];

    /// Which section to open on, from `FLOW_SECTION`.
    ///
    /// For iterating on one screen: the window restarts on every rebuild, and
    /// landing on Overview each time costs a click back to whatever is being
    /// worked on. Matched against the nav labels rather than a second list of
    /// names, which would drift the first time a section is renamed. An unset
    /// or unrecognised value opens Overview, same as always.
    fn initial() -> Self {
        std::env::var("FLOW_SECTION")
            .ok()
            .and_then(|wanted| Self::from_label(&wanted))
            .unwrap_or(Section::Overview)
    }

    fn from_label(name: &str) -> Option<Self> {
        Section::ALL.into_iter().find(|section| section.label().eq_ignore_ascii_case(name.trim()))
    }

    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::History => "History",
            Section::Dictation => "Dictation",
            Section::Audio => "Audio",
            Section::Vocabulary => "Vocabulary",
            Section::Models => "Models",
            Section::About => "About",
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Select(Section),
    PushToTalk(bool),
    Refine(bool),
    Terminal(bool),
    Denoise(bool),
    Autostart(bool),
    Duck(u32),
    OpenConfig,
    /// Reveal a path in the file manager. About's config and history rows.
    OpenPath(std::path::PathBuf),
    /// systemctl --user <verb> flow.service
    Service(&'static str),
    /// A service command finished away from the UI thread.
    ServiceFinished(&'static str, Result<(), String>),
    /// Start listening for the next chord the user presses.
    CaptureChord,
    /// A key arrived while capturing.
    Captured(Option<String>),
    CancelCapture,
    /// Put the chord back to what a fresh install uses.
    ResetChord,
    /// Delete the `.part` files a stopped install left behind.
    DiscardPartial,
    TypingTerm(String),
    AddTerm,
    RemoveTerm(usize),
    Daemon(daemon::Event),
    /// A frame went by; only delivered while something is moving.
    Tick(std::time::Instant),
    Hover(Option<Section>),
    /// Which transcript the pointer is over, so History can light the row and
    /// offer copy without those being permanent chrome.
    HoverEntry(Option<usize>),
    /// Put one transcript on the clipboard.
    Copy(usize),
    /// Start, or restart after a failure, the first-run install.
    BeginSetup,
    /// Go back through setup deliberately, from About.
    RerunSetup,
    /// One line from `flow install --porcelain`.
    SetupEvent(setup::Event),
    /// `flow probe` came back with where refining will run.
    Probed(Option<String>),
    /// Stop after the speech model and get on with it.
    SkipRefine,
    /// Leave setup for the console proper.
    FinishSetup,
    /// Automatic startup, or a retry from setup, finished.
    SetupStarted(Result<(), String>, bool),
    CheckUpdate,
    UpdateChecked(update::Status),
    InstallUpdate,
    UpdateInstalled(Result<String, String>),
}

struct Console {
    section: Section,
    daemon: daemon::State,
    settings: settings::Settings,
    /// Set when a save fails, so a read-only config or a full disk is visible
    /// rather than a control that silently springs back.
    save_error: Option<String>,
    /// A service failure belongs on Overview, beside the control that caused
    /// it, instead of in the settings-only save status.
    service_error: Option<String>,
    /// The service verb currently running. Kept separate from daemon activity:
    /// the socket may still report Offline while systemd is starting it.
    service_pending: Option<&'static str>,
    /// True once anything has been written. The daemon only reads its config at
    /// startup, so the window has to say so rather than imply a live change.
    saved: bool,
    /// None when systemd cannot answer - the control is hidden rather than
    /// shown in a state we cannot vouch for.
    autostart: Option<bool>,
    /// The microphone PipeWire is actually handing the daemon.
    input: Option<String>,
    entries: Vec<history::Entry>,
    /// Which row was last copied and when, so its button can say so and then
    /// go back to saying what it does.
    copied: Option<(usize, std::time::Instant)>,
    /// Per-day rollup for the Overview's calendar and week numbers, oldest
    /// day first.
    days: Vec<history::Day>,
    /// Result of the last update check. Starts Unknown: opening a settings
    /// window should not put a network call in the path of flipping a switch.
    update: update::Status,
    /// True while the release tarball is downloading and installing.
    updating: bool,
    models: Vec<system::Model>,
    /// Bytes sitting in unfinished `.part` downloads. Zero on a clean install,
    /// and then the Models screen says nothing about them.
    partial: u64,
    /// Some until the models are on disk. While it is set the window is the
    /// setup screen and nothing else - no rail, no sections. See `setup`.
    setup: Option<setup::State>,
    session: String,
    terms: Vec<String>,
    typing: String,
    term_error: Option<String>,
    /// Set only by `FLOW_CONSOLE_DEMO`, and then the window shows a daemon that
    /// is not there so the live states can be looked at. Also the reason the
    /// real socket's events are ignored: it would report Offline within
    /// seconds and undo the whole point.
    demo: bool,
    /// True while waiting for the user to press a new chord.
    capturing: bool,
    /// False when /dev/input cannot be read, so the chord cannot be captured.
    can_capture: bool,
    cancel_capture: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Why the last attempted chord was rejected, shown in place of the hint.
    chord_error: Option<String>,

    // Motion. Times rather than tweens: what a frame needs to know is how long
    // ago something changed, and everything here derives from that.
    now: std::time::Instant,
    hovered: Option<Section>,
    /// Transcript under the pointer, if any. Independent of the rail so a
    /// History hover cannot light a nav item, and the other way around.
    hovered_entry: Option<usize>,
    hover_at: std::time::Instant,
    /// This row's own clock, separate from the rail's `hover_at`: sharing one
    /// clock meant moving from a nav item onto a row (or back) restarted the
    /// row's fade from whatever point the nav's hover had reached, so the
    /// highlight sometimes never finished settling.
    entry_hover_at: std::time::Instant,
    /// When each toggle last flipped, so its knob can travel rather than jump.
    toggled_at: std::collections::HashMap<&'static str, std::time::Instant>,
}

impl Console {
    fn new() -> (Self, Task<Message>) {
        let entries = history::recent();
        let pretend = demo::requested();

        // A machine with no speech model cannot dictate, so the window has
        // nothing to report and one thing to do. Demo mode decides for itself,
        // so the screen can be laid out on a machine where the models are
        // present - or absent - either way.
        let first_run = match demo::setup() {
            Some(wanted) => wanted,
            None => setup::needed(),
        };

        (
            Self {
                section: Section::initial(),
                daemon: pretend.map_or_else(daemon::State::default, demo::daemon_state),
                settings: settings::Settings::load(),
                save_error: None,
                service_error: None,
                service_pending: None,
                saved: false,
                autostart: system::autostart_enabled(),
                input: match pretend {
                    Some(_) => demo::input(),
                    None => system::default_input(),
                },
                entries,
                copied: None,
                days: history::daily(CALENDAR_DAYS),
                update: update::Status::default(),
                updating: false,
                models: match pretend {
                    Some(_) => demo::models(),
                    None => system::models(),
                },
                setup: first_run.then(setup::State::default),
                partial: system::partial_bytes(),
                session: system::session(),
                terms: vocabulary::load(),
                typing: String::new(),
                term_error: None,
                demo: pretend.is_some(),
                capturing: false,
                can_capture: chord::available(),
                cancel_capture: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                chord_error: None,
                now: std::time::Instant::now(),
                hovered: None,
                hovered_entry: None,
                hover_at: std::time::Instant::now(),
                entry_hover_at: std::time::Instant::now(),
                toggled_at: std::collections::HashMap::new(),
            },
            // Setup starts itself. Launching Flow with nothing installed is
            // already the request; a Begin button in front of it would only be
            // asking the same question twice.
            if first_run { Task::done(Message::BeginSetup) } else { Task::none() },
        )
    }

    /// How far this toggle is through its travel, 0 to 1. A toggle that has
    /// never moved is already home.
    fn travel(&self, key: &str) -> f32 {
        self.toggled_at.get(key).map(|at| progress(*at, self.now, KNOB)).unwrap_or(1.0)
    }

    /// True while any motion is still running, which is what decides whether
    /// to ask for frames at all.
    fn moving(&self) -> bool {
        let running = |since: std::time::Instant, ms: u64| {
            self.now.saturating_duration_since(since).as_millis() < ms as u128
        };
        running(self.hover_at, FADE)
            || running(self.entry_hover_at, FADE)
            || self.copied.is_some_and(|(_, at)| running(at, COPIED))
            || self.toggled_at.values().any(|at| running(*at, KNOB))
            // The only motion in the product driven by something outside it:
            // the bar is chasing a download, so it moves until it catches up
            // rather than for a fixed duration.
            || self.setup.as_ref().is_some_and(setup::State::running)
    }

    /// True while this row's copy button should still be saying so.
    fn just_copied(&self, index: usize) -> bool {
        self.copied.is_some_and(|(i, at)| {
            i == index && self.now.saturating_duration_since(at).as_millis() < COPIED as u128
        })
    }

    /// 0 to 1, how far this transcript's hover has settled. Same easing and
    /// duration as the rail (`progress`, `FADE`), just its own clock.
    fn entry_warmth(&self, index: usize) -> f32 {
        if self.hovered_entry == Some(index) {
            progress(self.entry_hover_at, self.now, FADE)
        } else {
            0.0
        }
    }

    /// Write after every change. There is no Save button on purpose: a settings
    /// window with an unsaved state is a window that can lose your settings.
    fn persist(&mut self) {
        match self.settings.save() {
            Ok(()) => {
                self.save_error = None;
                self.saved = true;
            }
            Err(err) => self.save_error = Some(err.to_string()),
        }
    }

    fn start_setup_daemon(&mut self, close_when_started: bool) -> Task<Message> {
        let Some(state) = self.setup.as_mut() else {
            return Task::none();
        };
        if state.starting_daemon {
            return Task::none();
        }

        state.starting_daemon = true;
        state.start_error = None;
        Task::perform(async { system::service("start") }, move |result| {
            Message::SetupStarted(result, close_when_started)
        })
    }

    fn leave_setup(&mut self) {
        let rerun = self.setup.as_ref().is_some_and(|state| state.rerun);
        self.setup = None;
        self.models = system::models();
        self.partial = system::partial_bytes();
        self.input = system::default_input();
        self.entries = history::recent();
        self.autostart = system::autostart_enabled();

        // Someone who stopped the daemon and then repaired a model did not ask
        // for it back.
        if rerun {
            self.section = Section::About;
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // The socket is the one thing the demo cannot talk over: it is either
        // absent or belongs to a real daemon, and either way its first event
        // would replace the state being looked at.
        if self.demo && matches!(message, Message::Daemon(_)) {
            return Task::none();
        }

        match message {
            Message::Select(section) => self.section = section,
            Message::Tick(now) => {
                // The gap since the last frame, which is what the bar's easing
                // needs: a fraction-per-frame chase would settle at a different
                // speed on a 60Hz screen than on a 144Hz one.
                let elapsed = now.saturating_duration_since(self.now).as_secs_f32();
                self.now = now;
                if let Some(state) = self.setup.as_mut() {
                    state.advance(elapsed);
                }
            }
            Message::Hover(section) => {
                if self.hovered != section {
                    self.hovered = section;
                    self.hover_at = std::time::Instant::now();
                }
            }
            Message::HoverEntry(index) => {
                if self.hovered_entry != index {
                    self.hovered_entry = index;
                    self.entry_hover_at = std::time::Instant::now();
                }
            }
            Message::PushToTalk(on) => {
                self.settings.push_to_talk = on;
                self.toggled_at.insert("push_to_talk", std::time::Instant::now());
                self.persist();
            }
            Message::Refine(on) => {
                self.settings.refine = on;
                self.toggled_at.insert("refine", std::time::Instant::now());
                self.persist();
            }
            Message::Terminal(on) => {
                self.settings.terminal = on;
                self.toggled_at.insert("terminal", std::time::Instant::now());
                self.persist();
            }
            Message::Denoise(on) => {
                self.settings.denoise = on;
                self.toggled_at.insert("denoise", std::time::Instant::now());
                self.persist();
            }
            Message::Duck(value) => {
                self.settings.duck = value;
                self.persist();
            }
            Message::Autostart(on) => {
                self.toggled_at.insert("autostart", std::time::Instant::now());
                match system::set_autostart(on) {
                    // Re-read rather than assume: systemd is the authority on
                    // whether that worked, not our optimism.
                    Ok(()) => {
                        self.autostart = system::autostart_enabled();
                        self.save_error = None;
                    }
                    Err(err) => self.save_error = Some(err),
                }
            }
            Message::Daemon(daemon::Event::Line(line)) => {
                let before = self.daemon.words;
                self.daemon.apply(&line);
                // Re-read the file rather than trust the socket's copy: the
                // file is what this window shows, and it is the thing that
                // outlives the daemon.
                if self.daemon.words != before {
                    self.entries = history::recent();
                    self.days = history::daily(CALENDAR_DAYS);
                }
            }
            Message::Copy(index) => {
                if let Some(text) = self.entries.get(index).map(|entry| entry.text.clone()) {
                    self.copied = Some((index, std::time::Instant::now()));
                    return iced::clipboard::write(text);
                }
            }
            Message::Daemon(daemon::Event::Disconnected) => self.daemon = daemon::State::default(),
            Message::TypingTerm(text) => {
                self.typing = text;
                self.term_error = None;
            }
            Message::AddTerm => match vocabulary::validate(&self.typing, &self.terms) {
                Ok(term) => {
                    self.terms.push(term);
                    self.typing.clear();
                    self.term_error = None;
                    if let Err(err) = vocabulary::save(&self.terms) {
                        self.term_error = Some(err.to_string());
                    }
                }
                Err(why) => self.term_error = Some(why),
            },
            Message::RemoveTerm(index) => {
                if index < self.terms.len() {
                    self.terms.remove(index);
                    if let Err(err) = vocabulary::save(&self.terms) {
                        self.term_error = Some(err.to_string());
                    }
                }
            }
            Message::CaptureChord => {
                self.capturing = true;
                self.chord_error = None;
                let cancelled = std::sync::Arc::clone(&self.cancel_capture);
                cancelled.store(false, std::sync::atomic::Ordering::Relaxed);
                // Off the UI thread: this blocks on the keyboard until a chord
                // arrives or the user gives up.
                return Task::perform(
                    async move { tokio_free_capture(cancelled) },
                    Message::Captured,
                );
            }
            Message::DiscardPartial => {
                match system::discard_partials() {
                    Ok(()) => self.save_error = None,
                    Err(err) => self.save_error = Some(err),
                }
                self.partial = system::partial_bytes();
            }
            Message::ResetChord => {
                self.settings.hotkey = settings::DEFAULT_HOTKEY.to_string();
                self.chord_error = None;
                self.persist();
            }
            Message::CancelCapture => {
                self.capturing = false;
                self.cancel_capture.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Message::Captured(captured) => {
                self.capturing = false;
                // A None is a cancel, or no readable keyboard. The control is
                // hidden in the second case, so it is nearly always the first.
                if let Some(chord) = captured {
                    self.settings.hotkey = chord;
                    self.persist();
                }
            }
            Message::Service(verb) => {
                if self.service_pending.is_some() {
                    return Task::none();
                }
                self.service_pending = Some(verb);
                self.service_error = None;
                if matches!(verb, "start" | "restart") {
                    self.daemon.activity = daemon::Activity::Starting;
                }
                return Task::perform(async move { system::service(verb) }, move |result| {
                    Message::ServiceFinished(verb, result)
                });
            }
            Message::ServiceFinished(verb, result) => {
                self.service_pending = None;
                match result {
                    Ok(()) => {
                        self.service_error = None;
                        if verb == "stop" {
                            self.daemon = daemon::State::default();
                        }
                    }
                    Err(err) => {
                        self.service_error = Some(err);
                        if matches!(verb, "start" | "restart") {
                            self.daemon = daemon::State::default();
                        }
                    }
                }
            }
            Message::OpenConfig => {
                if let Err(err) = system::open(&settings::config_path()) {
                    self.save_error = Some(err);
                }
            }
            Message::OpenPath(path) => {
                if let Err(err) = system::reveal(&path) {
                    self.save_error = Some(err);
                }
            }
            Message::CheckUpdate => {
                if self.update != update::Status::Checking {
                    self.update = update::Status::Checking;
                    return Task::perform(async { update::latest() }, Message::UpdateChecked);
                }
            }
            Message::UpdateChecked(status) => self.update = status,
            Message::InstallUpdate => {
                if let (false, update::Status::Available(tag)) = (self.updating, &self.update) {
                    let tag = tag.clone();
                    self.updating = true;
                    self.save_error = None;
                    return Task::perform(
                        async move { update::install(&tag).map(|()| tag) },
                        Message::UpdateInstalled,
                    );
                }
            }
            Message::UpdateInstalled(result) => {
                self.updating = false;
                match result {
                    Ok(tag) => self.update = update::Status::Installed(tag),
                    Err(err) => self.save_error = Some(err),
                }
            }
            // A rerun keeps whatever the screen already knew about the machine,
            // so pressing Run setup from About does not blank the hardware line
            // it is about to show again.
            Message::BeginSetup | Message::RerunSetup => {
                let rerun = matches!(message, Message::RerunSetup);

                // Demo mode never spawns the installer: the point of it is to
                // lay this screen out on a machine that has no `flow` binary
                // at all, and a failure line is not the state worth looking at.
                if self.demo {
                    self.setup = Some(setup::State { rerun, ..demo::setup_state() });
                    return Task::none();
                }

                let (events, handle) = setup::install(false);
                self.setup = Some(setup::State { handle, rerun, ..setup::State::default() });
                return Task::batch([
                    Task::run(events, Message::SetupEvent),
                    // Asked once, alongside the download rather than before it:
                    // `flow probe` initialises Vulkan to enumerate cards, and
                    // nothing should stand between launching Flow and the first
                    // byte arriving.
                    Task::perform(async { setup::probe() }, Message::Probed),
                ]);
            }
            Message::SetupEvent(event) => {
                let should_start = if let Some(state) = self.setup.as_mut() {
                    let was_done = state.phase == setup::Phase::Done;
                    state.apply(event);
                    !was_done && state.phase == setup::Phase::Done && !state.rerun
                } else {
                    false
                };

                // The download completing is the end of setup, so the daemon
                // starts here rather than waiting for a navigation button.
                if should_start {
                    return self.start_setup_daemon(false);
                }
            }
            Message::Probed(hardware) => {
                if let Some(state) = self.setup.as_mut() {
                    state.hardware = hardware;
                }
            }
            Message::SkipRefine => {
                if let Some(state) = self.setup.as_mut() {
                    // Marked before the kill, so the failure the reader is
                    // about to report is read as the skip it is.
                    state.skipped = true;
                    state.handle.stop();
                }
            }
            Message::SetupStarted(result, close_when_started) => {
                let started = result.is_ok();
                if let Some(state) = self.setup.as_mut() {
                    state.starting_daemon = false;
                    match result {
                        Ok(()) => {
                            state.daemon_started = true;
                            state.start_error = None;
                        }
                        Err(err) => state.start_error = Some(err),
                    }
                }

                if started {
                    self.daemon.activity = daemon::Activity::Starting;
                    if close_when_started {
                        self.leave_setup();
                    }
                }
            }
            Message::FinishSetup => {
                let ready_to_leave =
                    self.setup.as_ref().is_some_and(|state| state.rerun || state.daemon_started);
                if ready_to_leave {
                    self.leave_setup();
                } else {
                    // Automatic startup may have failed. The completion button
                    // is the focused retry and only leaves setup after systemd
                    // confirms that Flow stayed running.
                    return self.start_setup_daemon(true);
                }
            }
        }
        Task::none()
    }

    /// The line under a settings screen. The daemon watches the config file, so
    /// almost everything here is live and the note says so; the exceptions name
    /// themselves on their own row rather than making every screen apologise.
    fn save_note(&self) -> Element<'_, Message> {
        match (&self.save_error, self.saved) {
            (Some(err), _) => text(format!("Couldn't save: {err}")).size(12).color(ERR),
            (None, true) => text("Saved. Applies to your next dictation.").size(12).color(FAINT),
            (None, false) => text(settings::config_path().display().to_string())
                .size(12)
                .font(Font::MONOSPACE)
                .color(FAINT),
        }
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        // Setup takes the whole window, rail included. The rail is a way to
        // move between seven screens that have nothing on them yet, and
        // offering it here would be offering the user seven ways to watch the
        // same download from somewhere it cannot be seen.
        if let Some(state) = &self.setup {
            return setup::view(state);
        }

        // The rail/pane divider is its own element: a container border applies
        // to all four sides, and only this edge should be drawn.
        row![self.rail(), vertical_hairline(), self.pane()].into()
    }

    // -- rail ---------------------------------------------------------------

    fn rail(&self) -> Element<'_, Message> {
        let mut items = column![
            container(text("Flow").size(14).color(FG)).padding([0, 9]),
            Space::new().height(14)
        ]
        .spacing(2);

        for section in Section::ALL {
            let selected = section == self.section;
            let warmth = if self.hovered == Some(section) {
                progress(self.hover_at, self.now, FADE)
            } else {
                0.0
            };
            items = items.push(nav(section, selected, warmth));
        }

        container(
            column![
                items,
                Space::new().height(Fill),
                container(
                    text(env!("CARGO_PKG_VERSION")).size(11).font(Font::MONOSPACE).color(FAINT)
                )
                .padding([0, 9]),
            ]
            .spacing(0),
        )
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Fill)
        .padding([26, 11])
        .into()
    }

    // -- pane ---------------------------------------------------------------

    fn pane(&self) -> Element<'_, Message> {
        let content = match self.section {
            Section::Overview => self.overview_section(),
            Section::History => self.history_section(),
            Section::Dictation => self.dictation_section(),
            Section::Audio => self.audio_section(),
            Section::Vocabulary => self.vocabulary_section(),
            Section::Models => self.models_section(),
            Section::About => self.about_section(),
        };

        // Switching sections is deliberately instant. Motion here read as the
        // page arriving late rather than as polish - navigation should feel
        // like the content was already there.
        //
        // Left only, not both sides: a right pad here would sit outside the
        // scrollable and push its whole viewport away from the window edge,
        // floating the scrollbar in a dead gutter instead of letting it run
        // where every other app puts one - flush against the edge it scrolls.
        // `scroll_pad` owns the right side, on the content, so the bar can
        // sit at the edge while the text keeps its own margin from it.
        let left = if matches!(self.section, Section::History) {
            PANE_INSET - ENTRY_INSET
        } else {
            PANE_INSET
        };

        container(content)
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding::default().left(left))
            .into()
    }

    /// The landing page, and the only one that is a report rather than a form.
    ///
    /// Laid out to answer three questions in the order they get asked: is Flow
    /// working (the status card, with the three facts that decide it), has it
    /// been used (a week of numbers, each against something to compare it to),
    /// and is it any good (the calendar, then the last few things it actually
    /// wrote). A bare number answers none of those on its own, which is why
    /// every tile here carries a second line.
    ///
    /// The heading carries the status and its action, rather than the page
    /// having a heading *and* a card whose whole top half is a heading of its
    /// own. That was the shape that felt unfinished: two stacked title rows,
    /// and a card left holding a status line and three facts that have nothing
    /// to do with each other. What is left below the heading is one thing -
    /// the settings that decide whether Flow can work - and it is also what
    /// buys back the height the heading costs.
    fn overview_section(&self) -> Element<'_, Message> {
        let running = self.daemon.activity != daemon::Activity::Offline;
        let installed = self.models.iter().filter(|m| m.installed).count();
        let (label, dot) = activity_label(self.daemon.activity);
        let service_action: Element<'_, Message> = match self.service_pending {
            Some("start") => container(text("Starting…").size(13).color(MUTED))
                .padding([7, 14])
                .into(),
            Some("restart") => container(text("Restarting…").size(13).color(MUTED))
                .padding([7, 14])
                .into(),
            Some(_) => container(text("Working…").size(13).color(MUTED))
                .padding([7, 14])
                .into(),
            None => action_msg(
                if running { "Restart" } else { "Start" },
                !running,
                Message::Service(if running { "restart" } else { "start" }),
            ),
        };

        let header = row![
            text("Overview").size(22).color(FG),
            Space::new().width(Fill),
            pip(dot),
            Space::new().width(9),
            text(label).size(13).color(MUTED),
            Space::new().width(16),
            service_action,
        ]
        .align_y(iced::Center);

        // The last two weeks, so every number on the KPI row has something to
        // be measured against. The buffer is two years, so these always exist
        // - a fresh install simply has zeros in them.
        let total = self.days.len();
        let this_week = &self.days[total - 7..];
        let last_week = &self.days[total - 14..total - 7];

        let spoken: f32 = this_week.iter().map(|day| day.spoken).sum();
        let dictations: u32 = this_week.iter().map(|day| day.dictations).sum();
        let active = this_week.iter().filter(|day| day.dictations > 0).count();
        let streak = current_streak(&self.days);

        let kpis = row![
            stat_tile(
                "Words this week",
                commas(history::words(this_week)),
                trend(history::words(this_week), history::words(last_week)),
            ),
            Space::new().width(GAP),
            stat_tile(
                "Dictations",
                dictations.to_string(),
                (format!("{active} of 7 days active"), MUTED),
            ),
            Space::new().width(GAP),
            stat_tile(
                "Speaking time",
                history::duration(spoken),
                if dictations == 0 {
                    ("nothing this week".to_string(), MUTED)
                } else {
                    (format!("{} average", history::duration(spoken / dictations as f32)), MUTED)
                },
            ),
            Space::new().width(GAP),
            stat_tile(
                "Current streak",
                plural(streak as u32, "day"),
                (format!("longest {}", plural(longest_streak(&self.days) as u32, "day")), MUTED,),
            ),
        ];

        // Sized to fit the default window, so the common case does not scroll
        // at all. Scrolled rather than compressed when it does not fit: a user
        // who drags the window down to the minimum height should have to reach
        // the last card rather than have every card shrink to meet them. The
        // heading is in the scroll, same as every other page: a window this
        // short has to give the cards the room, not keep a title parked over
        // them.
        scroll(column![
            header,
            Space::new().height(SCROLL_PAD),
            self.setup_card(installed),
            Space::new().height(GAP),
            kpis,
            Space::new().height(GAP),
            calendar_card(&self.days),
        ])
    }

    /// The three settings that decide whether Flow can work at all, and
    /// anything currently wrong above them.
    ///
    /// They are here rather than only in their own sections because "it isn't
    /// typing anything" is nearly always one of the three being wrong, and
    /// this is the page someone opens to find that out. Packed left with real
    /// gaps rather than spread across equal columns: three short values in
    /// three wide columns is mostly voids, and a row of voids is what makes a
    /// card look unfinished.
    fn setup_card(&self, installed: usize) -> Element<'_, Message> {
        let facts = row![
            fact(
                "Chord",
                if self.settings.push_to_talk {
                    self.settings.hotkey.replace('+', " ")
                } else {
                    "push to talk is off".to_string()
                },
            ),
            Space::new().width(44),
            fact("Microphone", clip(self.input.as_deref().unwrap_or("system default"), 38,),),
            Space::new().width(Fill),
            fact("Models", format!("{installed} of {}", self.models.len())),
        ];

        let mut body = column![];
        let notes = self.attention(installed);
        // Notes sit close together - they are one list of problems - and the
        // rule under them takes `ROW_PAD` on both sides, the same as every
        // other divider in the console. Borrowing a tighter gap above it and
        // a wider one below made the card's two rules read as different
        // rules on a page where they are the same one.
        for (index, (colour, note)) in notes.iter().enumerate() {
            if index > 0 {
                body = body.push(Space::new().height(8));
            }
            body = body.push(text(note.clone()).size(12).color(*colour));
        }
        if !notes.is_empty() {
            body = body.push(Space::new().height(ROW_PAD));
            body = body.push(card_rule());
            body = body.push(Space::new().height(ROW_PAD));
        }

        body = body.push(self.last_said());
        body = body.push(Space::new().height(ROW_PAD));
        body = body.push(card_rule());
        body = body.push(Space::new().height(ROW_PAD));

        panel(body.push(facts).into())
    }

    /// The last thing Flow typed, as one line.
    ///
    /// This used to be a card of its own holding two full History rows, copy
    /// controls included, and a link back to the page those rows came from -
    /// History rebuilt on the page next to History. What it was actually for
    /// survives here and the duplicate does not: a word count says dictation
    /// ran, and a sentence says it ran *well*, which is the question someone
    /// opens this page with. Reading the log, copying out of it and scrolling
    /// back through it stay History's job, one item down the rail.
    fn last_said(&self) -> Element<'_, Message> {
        let latest = self.entries.first();
        let when = latest.map(|entry| history::ago(entry.at, history::now())).unwrap_or_default();

        let heading = row![
            text("Last dictation").size(11).color(FAINT),
            Space::new().width(Fill),
            // `ago` already says "just now" for the last minute; empty means
            // the timestamp is missing or in the future, and no label is
            // better than a confident wrong one.
            text(when)
                .size(11)
                .font(Font::MONOSPACE)
                .color(FAINT),
        ]
        .align_y(iced::Center);

        // Clipped to one line on purpose: the full text, wrapped, is a
        // transcript, and a transcript on this page is the card that just got
        // deleted. One line is enough to recognise what was said and how well
        // it was heard.
        let line = match latest {
            Some(entry) => text(clip(&entry.text, 78))
                .size(12.5)
                .color(mix(MUTED, FG, 0.55))
                .wrapping(text::Wrapping::None),
            None => text("nothing yet - hold the chord and say something").size(12.5).color(FAINT),
        };

        column![heading, Space::new().height(5), line].into()
    }

    /// Everything on this page that is worth doing something about, in the
    /// order it would bite. Empty when there is nothing to do, and the status
    /// card then collapses to one line - a dashboard that always has a row of
    /// warnings in it teaches people not to read the warnings.
    fn attention(&self, installed: usize) -> Vec<(Color, String)> {
        let mut notes = Vec::new();
        if let Some(problem) = &self.service_error {
            notes.push((ERR, problem.clone()));
        }
        if let Some(problem) = &self.daemon.problem {
            notes.push((ERR, problem.clone()));
        }
        if installed < self.models.len() {
            notes.push((
                ERR,
                "Models are missing - Flow cannot transcribe until they install.".to_string(),
            ));
        }
        if let update::Status::Available(tag) = &self.update {
            notes.push((ACCENT, format!("{tag} is available to install.")));
        }
        if self.saved {
            notes.push((
                MUTED,
                "Settings changed - restart Flow for them to take effect.".to_string(),
            ));
        }
        notes
    }

    fn history_section(&self) -> Element<'_, Message> {
        let now = history::now();

        let list: Element<'_, Message> = if self.entries.is_empty() {
            // Sits on the heading's left edge and at a row's own top pad, so
            // the line reads as the first entry's place rather than as loose
            // text floating outside the list.
            container(text("Nothing yet. Hold the chord and say something.").size(13).color(FAINT))
                .padding([10.0, ENTRY_INSET])
                .width(Fill)
                .into()
        } else {
            let mut rows = column![];
            for (index, entry) in self.entries.iter().enumerate() {
                rows = rows.push(entry_row(
                    entry,
                    index,
                    now,
                    self.just_copied(index),
                    self.entry_warmth(index),
                    index + 1 < self.entries.len(),
                ));
            }
            entry_list(rows)
        };

        scroll_inset(
            column![
                container(heading(
                    "History",
                    "Everything Flow has typed for you, most recent first.",
                ))
                .padding([0.0, ENTRY_INSET])
                .width(Fill),
                list,
            ],
            PAGE_TOP,
            CONTENT_RIGHT - ENTRY_INSET,
        )
    }

    fn dictation_section(&self) -> Element<'_, Message> {
        let mut rows: Vec<Element<Message>> = vec![
            setting(
                "Push to talk",
                "Flow watches the chord itself, so no compositor binding is needed. Turning it on needs a restart.",
                toggle(self.settings.push_to_talk, self.travel("push_to_talk"), Message::PushToTalk),
            ),
            setting(
                "Chord",
                // Not "applies straight away", which it never did: the daemon
                // reads the chord once at startup because the thread watching
                // it would have to be torn down and rebuilt. Saying otherwise
                // sent people off pressing a combination that was never going
                // to fire, and blaming their keyboard for it.
                "Held down while you speak. Restart Flow for a change to take effect.",
                row![
                    text(if self.capturing {
                        "press the chord…".to_string()
                    } else {
                        self.settings.hotkey.replace('+', " ")
                    })
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(if self.capturing { ACCENT } else { MUTED }),
                    Space::new().width(12),
                    // Reset earns its place only when the chord is not
                    // already the default - offered next to a chord that is
                    // the default, it is a button that does nothing.
                    if !self.capturing && self.settings.hotkey != settings::DEFAULT_HOTKEY {
                        row![
                            action_msg("Reset", false, Message::ResetChord),
                            Space::new().width(8)
                        ]
                        .into()
                    } else {
                        Element::from(Space::new().width(0))
                    },
                    if self.capturing {
                        action_msg("Cancel", false, Message::CancelCapture)
                    } else if self.can_capture {
                        action_msg("Change", false, Message::CaptureChord)
                    } else {
                        // No readable keyboard, so offer the file instead of a
                        // button that could only fail.
                        action_msg("Open config", false, Message::OpenConfig)
                    },
                ]
                .align_y(iced::Center)
                .into(),
            ),
            setting(
                "Refine transcript",
                "Removes filler and fixes punctuation with the local model. Turning it back on needs a restart.",
                toggle(self.settings.refine, self.travel("refine"), Message::Refine),
            ),
            setting(
                "Terminal paste chord",
                "Send Ctrl+Shift+V when a terminal has focus.",
                toggle(self.settings.terminal, self.travel("terminal"), Message::Terminal),
            ),
            setting(
                "Vocabulary",
                "Names and jargon the recogniser should get right.",
                row![
                    text(format!("{} terms", self.terms.len()))
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(MUTED),
                    Space::new().width(12),
                    action_msg("Edit", false, Message::Select(Section::Vocabulary)),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        ];

        // Only offered when systemd actually answered. A switch we cannot read
        // the true state of is worse than no switch.
        if let Some(enabled) = self.autostart {
            rows.push(setting(
                "Start with session",
                "Enables the flow.service user unit so the daemon launches when you log in.",
                toggle(enabled, self.travel("autostart"), Message::Autostart),
            ));
        }

        section_shell(
            "Dictation",
            "How the chord behaves and what happens to your words.",
            rows,
            Some(self.save_note()),
        )
    }

    fn audio_section(&self) -> Element<'_, Message> {
        let rows: Vec<Element<Message>> = vec![
            // Read-only, and deliberately so: the daemon records from the
            // system default source, which means changing your microphone in
            // your desktop's own settings already works. A picker here could
            // only ever be a second answer to the same question.
            setting(
                "Microphone",
                "Follows your system's default input. Change it in your sound settings.",
                text(self.input.clone().unwrap_or_else(|| "not detected".to_string()))
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(MUTED)
                    .into(),
            ),
            setting(
                "Turn other apps down",
                "Keeps your speakers out of the microphone while you dictate.",
                value_slider(
                    0..=100,
                    self.settings.duck,
                    Message::Duck,
                    &format!("{}%", self.settings.duck),
                ),
            ),
            setting(
                "Noise suppression",
                "Runs RNNoise over the audio. Can blunt consonants on a weak mic.",
                toggle(self.settings.denoise, self.travel("denoise"), Message::Denoise),
            ),
        ];

        section_shell(
            "Audio",
            "What Flow listens to, and what it does to the room first.",
            rows,
            None,
        )
    }

    /// The vocabulary, edited here rather than in a text editor. The file is
    /// the daemon's interface; it should not have to be the user's.
    fn vocabulary_section(&self) -> Element<'_, Message> {
        let mut list = column![];
        if self.terms.is_empty() {
            list = list.push(
                // Sits where the first term would, so the line reads as the
                // list's own state rather than as a third paragraph of help.
                container(
                    text("No terms yet. Add the words Flow keeps mishearing.")
                        .size(13)
                        .color(FAINT),
                )
                .padding([ROW_PAD, 0.0]),
            );
        } else {
            for (index, term) in self.terms.iter().enumerate() {
                list = list.push(
                    container(
                        row![
                            text(term.clone()).size(13).color(FG),
                            Space::new().width(Fill),
                            action_msg("Remove", false, Message::RemoveTerm(index)),
                        ]
                        .align_y(iced::Center),
                    )
                    // The same air as any other row in this console, on both
                    // sides of every hairline. At 6 the terms read as a
                    // cramped table dropped into a page whose every other
                    // list breathes.
                    .padding([ROW_PAD, 0.0]),
                );
                if index + 1 < self.terms.len() {
                    list = list.push(hairline());
                }
            }
        }

        let entry = row![
            iced::widget::text_input("Hyprland", &self.typing)
                .on_input(Message::TypingTerm)
                .on_submit(Message::AddTerm)
                .size(13)
                .padding([8, 10])
                .style(|_theme, _status| iced::widget::text_input::Style {
                    background: Background::Color(BG),
                    border: Border { color: LINE, width: 1.0, radius: 6.0.into() },
                    icon: FAINT,
                    placeholder: FAINT,
                    value: FG,
                    selection: ACCENT,
                }),
            Space::new().width(8),
            action_msg("Add", true, Message::AddTerm),
        ]
        .align_y(iced::Center);

        let note: Element<Message> = match &self.term_error {
            Some(why) => text(why.clone()).size(12).color(ERR).into(),
            None => text(
                "Flow only fixes words that sound close to what you said: \
                 \"hyper land\" becomes Hyprland. It cannot rescue a name it \
                 heard as something unrelated.",
            )
            .size(12)
            .color(FAINT)
            .into(),
        };

        scroll(column![
            heading(
                "Vocabulary",
                "Names and jargon the recogniser gets wrong. One per line, spelled the way you want it written.",
            ),
            entry,
            // Tight to the field it explains, then a real gap before the
            // list - the two spaces have to differ or the field, its note
            // and the terms read as three unrelated things equally spaced.
            Space::new().height(8),
            note,
            Space::new().height(GAP),
            list,
        ])
    }

    fn models_section(&self) -> Element<'_, Message> {
        // Bound so the borrows in the rows outlive their construction.
        let sizes: Vec<String> =
            self.models.iter().map(|model| system::human_bytes(model.bytes)).collect();

        let rows: Vec<Element<Message>> = self
            .models
            .iter()
            .zip(&sizes)
            .map(|(model, size)| {
                model_row(model.label, model.detail.clone(), size.clone(), model.installed)
            })
            .collect();

        // Only when there is something stranded. A row explaining that you
        // have no unfinished downloads would be a row about nothing.
        let mut rows = rows;
        if self.partial > 0 {
            rows.push(self.partial_row());
        }

        let total = format!(
            "{} in {}",
            system::human_bytes(self.models.iter().map(|m| m.bytes).sum()),
            flow_paths::models_dir().display()
        );
        let all_installed = self.models.iter().all(|model| model.installed);

        section_shell(
            "Models",
            "Both models run on this machine. Nothing you say leaves it.",
            rows,
            Some(path_cta(
                text(total).size(12).font(Font::MONOSPACE).color(FAINT).into(),
                (!all_installed).then(|| {
                    action_msg(
                        "Install models",
                        true,
                        // The same screen the first run uses, so a download
                        // here gets the same progress bar rather than a button
                        // that says "Installing…" for twenty minutes.
                        Message::RerunSetup,
                    )
                }),
            )),
        )
    }

    /// What a skipped or interrupted download left behind, and the way to be
    /// rid of it.
    ///
    /// Kept by default because curl resumes from it, so changing your mind
    /// costs only the bytes that never arrived. This row exists for the other
    /// case: deciding you never wanted the model at all, which the file itself
    /// has no way of knowing.
    fn partial_row(&self) -> Element<'_, Message> {
        container(
            row![
                column![
                    text("Unfinished download").size(13.5).color(FG),
                    Space::new().height(3),
                    text(
                        "From an install that stopped early. Installing again resumes from it \
                         instead of starting over.",
                    )
                    .size(12)
                    .color(FAINT),
                ]
                .width(Length::FillPortion(3)),
                Space::new().width(20),
                container(
                    row![
                        text(system::human_bytes(self.partial))
                            .size(12)
                            .font(Font::MONOSPACE)
                            .color(FAINT),
                        Space::new().width(14),
                        action_msg("Discard", false, Message::DiscardPartial),
                    ]
                    .align_y(iced::Center),
                )
                .width(Length::FillPortion(2))
                .align_x(iced::alignment::Horizontal::Right),
            ]
            .align_y(iced::Center),
        )
        .padding([ROW_PAD, 0.0])
        .into()
    }

    fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
            self.setup_row(),
            fact_row("Session", self.session.clone()),
            fact_path("Config", &settings::config_path()),
            fact_path("History", &history::path()),
        ];

        section_shell(
            "Flow",
            "Push-to-talk dictation that runs entirely on your own machine.",
            rows,
            None,
        )
    }

    /// The way back to the first-run screen.
    ///
    /// It is not only for a machine that never finished: the installer verifies
    /// every file against its pinned hash, so running it again repairs a model
    /// that was interrupted, truncated or deleted, and re-reads which card the
    /// refining will use. That makes it the honest answer to "something is
    /// wrong with my models" as well as to "I skipped that, I want it now".
    fn setup_row(&self) -> Element<'_, Message> {
        container(
            row![
                column![
                    text("Setup").size(13.5).color(FG),
                    Space::new().height(3),
                    text("Check both models against their hashes, and fetch whatever is missing.")
                        .size(12)
                        .color(FAINT),
                ]
                .width(Length::FillPortion(3)),
                Space::new().width(20),
                container(action_msg("Run setup", false, Message::RerunSetup))
                    .width(Length::FillPortion(2))
                    .align_x(iced::alignment::Horizontal::Right),
            ]
            .align_y(iced::Center),
        )
        .padding([ROW_PAD, 0.0])
        .into()
    }

    /// What is running, whether anything newer exists, and the one button that
    /// acts on the answer - all on the row that already says which version this
    /// is. An update check belongs beside the version it is about, not on a
    /// second row explained by a button in the far corner of the pane.
    fn version_row(&self) -> Element<'_, Message> {
        let (dot, note) = update_state(&self.update);

        let action = if self.updating {
            action_msg("Updating…", true, Message::InstallUpdate)
        } else if let update::Status::Available(tag) = &self.update {
            action_msg(&format!("Update to {tag}"), true, Message::InstallUpdate)
        } else if self.update == update::Status::Checking {
            action_msg("Checking…", false, Message::CheckUpdate)
        } else {
            action_msg("Check for updates", false, Message::CheckUpdate)
        };

        container(
            row![
                column![
                    text("Version").size(13.5).color(FG),
                    Space::new().height(3),
                    text(note).size(12).color(FAINT),
                ],
                Space::new().width(Fill),
                pip(dot),
                Space::new().width(7),
                text(update::running()).size(12).font(Font::MONOSPACE).color(MUTED),
                Space::new().width(12),
                action,
            ]
            .align_y(iced::Center),
        )
        .padding([ROW_PAD, 0.0])
        .into()
    }
}

/// The line and dot colour for each activity the daemon can report. Offline
/// and Ready both read as calm (no accent) - the accent is reserved for the
/// two states where Flow is actually doing something with your voice.
fn activity_label(activity: daemon::Activity) -> (&'static str, Color) {
    match activity {
        daemon::Activity::Offline => ("Flow isn't running", FAINT),
        daemon::Activity::Starting => ("Starting…", MUTED),
        daemon::Activity::Ready => ("Flow is ready", FAINT),
        daemon::Activity::Listening => ("Listening", ACCENT),
        daemon::Activity::Working => ("Refining your words", ACCENT),
    }
}

/// The dot's colour and the line beside it, for each thing an update check can
/// come back with. The dot is the glance - green nothing to do, accent
/// something to install, red something went wrong - and the line is the detail
/// for whoever looks closer.
fn update_state(status: &update::Status) -> (Color, String) {
    match status {
        update::Status::Unknown => (FAINT, "not checked for updates yet".into()),
        update::Status::Checking => (MUTED, "checking for updates…".into()),
        update::Status::Current => (OK, "up to date".into()),
        update::Status::Available(tag) => (ACCENT, format!("{tag} is available")),
        update::Status::Installed(tag) => (OK, format!("{tag} installed - restart Flow to run it")),
        update::Status::Failed(why) => (ERR, format!("could not check: {why}")),
    }
}

/// Read the keyboard until a chord arrives, on whatever thread the runtime
/// gives us. Split out so the async block above stays a one-liner.
fn tokio_free_capture(cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<String> {
    chord::capture(&|| cancelled.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::Section;

    /// The lookup reads the nav labels, so renaming a section would otherwise
    /// silently turn FLOW_SECTION into "open Overview" with nothing to say so.
    #[test]
    fn every_section_can_be_named() {
        for section in Section::ALL {
            assert_eq!(
                Section::from_label(section.label()),
                Some(section),
                "{} cannot be reached by name",
                section.label()
            );
        }
    }

    #[test]
    fn the_name_is_forgiving_but_not_a_guess() {
        assert_eq!(Section::from_label("  audio "), Some(Section::Audio));
        assert_eq!(Section::from_label("AUDIO"), Some(Section::Audio));
        assert_eq!(Section::from_label("aud"), None);
        assert_eq!(Section::from_label(""), None);
    }
}
