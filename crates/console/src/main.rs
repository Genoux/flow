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
    entry_list, entry_row, fact_path, fact_row, heading, inert, nav, scroll, scroll_inset,
    section_shell, setting,
};
use crate::theme::{
    mix, progress, ACCENT, BG, CALENDAR_DAYS, CONTENT_RIGHT, COPIED, ENTRY_INSET, ERR, FADE, FAINT,
    FG, GAP, KNOB, LABEL_GAP, LINE, MUTED, OK, PAGE_TOP, PANE_INSET, RAIL_WIDTH, ROW_PAD,
    SCROLL_PAD, STARTING,
};
use iced::widget::{column, container, row, stack, text, Space};
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
    iced::theme::Style {
        background_color: BG,
        text_color: FG,
    }
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

/// The longest step any animation may be advanced by in one frame, in seconds.
/// Two frames at 30Hz - long enough that a slow compositor still eases at real
/// time, short enough that the idle gap before an animation starts cannot be
/// spent all at once. See `Message::Tick`.
const FRAME_CAP: f32 = 1.0 / 30.0;

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
    /// What used to be Models. Both models now arrive with the install, so the
    /// screen that asked which to fetch has no question left on it - what it
    /// has instead is the one choice that changes what Flow writes.
    Style,
    About,
}

impl Section {
    const ALL: [Section; 7] = [
        Section::Overview,
        Section::History,
        Section::Dictation,
        Section::Audio,
        Section::Vocabulary,
        Section::Style,
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
        Section::ALL
            .into_iter()
            .find(|section| section.label().eq_ignore_ascii_case(name.trim()))
    }

    /// Whether this screen still means something when Flow cannot run.
    ///
    /// Both models are required, so a machine missing either has no daemon at
    /// all - not a daemon doing less. That makes every screen that tunes
    /// dictation a screen tuning nothing: Dictation, Audio, Vocabulary and
    /// Style all describe behaviour that has no process to belong to.
    ///
    /// Overview survives because it is where the way out is, and About because
    /// a version and a path are true whether or not anything is running.
    ///
    /// Disabled rather than hidden. A nav that grows items when a download
    /// finishes is a nav that was lying about what the product is.
    fn works_without_models(self) -> bool {
        matches!(self, Section::Overview | Section::About)
    }

    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::History => "History",
            Section::Dictation => "Dictation",
            Section::Audio => "Audio",
            Section::Vocabulary => "Vocabulary",
            Section::Style => "Style",
            Section::About => "About",
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Select(Section),
    PushToTalk(bool),
    /// A cleanup card on the Style screen. Picking a level is the whole of that
    /// screen, so it saves immediately rather than behind a confirm.
    SetCleanup(settings::Cleanup),
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
    /// Throw the models away and run setup again from nothing.
    RerunSetup,
    /// Stop the download that is running and throw away what it had.
    StopDownload,
    /// One line from `flow install --porcelain`.
    SetupEvent(setup::Event),
    /// Automatic startup, or a retry from setup, finished.
    SetupStarted(Result<(), String>),
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
    /// True once anything has been written, which is what the footer's "Saved"
    /// note is for.
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
    /// The one model download that can be in flight, whether it is first
    /// run's or one started from a row on the Models screen.
    download: Option<setup::State>,
    /// True while the setup screen owns the whole window - no rail, no
    /// sections.
    showing_setup: bool,
    /// Seconds into setup dissolving away, once its work is done. `None` the
    /// rest of the time.
    fading: Option<f32>,
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

        let settings = settings::Settings::load();

        (
            Self {
                section: Section::initial(),
                daemon: pretend.map_or_else(daemon::State::default, demo::daemon_state),
                settings,
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
                // Already checking, because it is: the check goes out with
                // this window. Left at Unknown, About would read "not checked
                // yet" while a check was in flight and its button would fire a
                // second one.
                update: match pretend {
                    Some(_) => update::Status::Unknown,
                    None => update::Status::Checking,
                },
                updating: false,
                models: match pretend {
                    Some(_) => demo::models(),
                    None => system::models(),
                },
                download: None,
                showing_setup: first_run,
                fading: None,
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
            Task::batch([
                if first_run {
                    Task::done(Message::BeginSetup)
                } else {
                    Task::none()
                },
                // Whether there is a newer Flow, asked without being asked.
                // A release nobody knows about is a release nobody installs,
                // and the answer belongs on screen before the question occurs
                // to anyone - the Overview names an available version among
                // its notes, so opening the window is enough to hear about it.
                //
                // Quiet when it goes wrong: a check that fails says so on the
                // About screen and nowhere else, so a laptop with no network
                // opens on exactly the window it opened on before.
                if pretend.is_some() {
                    Task::none()
                } else {
                    Task::perform(async { update::latest() }, Message::UpdateChecked)
                },
            ]),
        )
    }

    /// How far this toggle is through its travel, 0 to 1. A toggle that has
    /// never moved is already home.
    fn travel(&self, key: &str) -> f32 {
        self.toggled_at
            .get(key)
            .map(|at| progress(*at, self.now, KNOB))
            .unwrap_or(1.0)
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
            || self.fading.is_some()
            // The only motion in the product driven by something outside it:
            // the ring is chasing a download, so it moves until it catches up
            // rather than for a fixed duration.
            || self.download.as_ref().is_some_and(setup::State::running)
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

    fn start_setup_daemon(&mut self) -> Task<Message> {
        let Some(state) = self.download.as_mut() else {
            return Task::none();
        };
        if state.starting_daemon {
            return Task::none();
        }

        state.starting_daemon = true;
        state.start_error = None;
        // `restart`, not `start`. Two reasons, and both are the same reason.
        //
        // A daemon that was already up started before these models existed, so
        // it is running without the one setup just fetched - `start` on an
        // active unit does nothing at all, and leaves it that way.
        //
        // And because it does nothing, the socket never drops, so the console
        // never reconnects and never gets the fresh snapshot that would correct
        // the "Starting…" it just set on itself. It sat there until something
        // else restarted the daemon. `restart` on an inactive unit simply
        // starts it, so this is right whether or not one was running.
        Task::perform(async { system::service("restart") }, Message::SetupStarted)
    }

    /// Spawn `flow install` once the intro has landed. Hashing during the
    /// fade is what made the motion hitch.
    fn launch_install(&mut self) -> Task<Message> {
        let Some(state) = self.download.as_mut() else {
            return Task::none();
        };
        if self.demo || state.spawned || state.stopped || !state.intro_over() {
            return Task::none();
        }
        state.spawned = true;
        let (events, handle) = setup::install();
        state.handle = handle;
        Task::run(events, Message::SetupEvent)
    }

    /// The speech model is on disk, so Flow can dictate: start the daemon.
    ///
    /// There is no button for this and nothing to confirm. Called from
    /// everything that can move setup forward, because what it is waiting on
    /// last is sometimes only `FLOOR`.
    fn setup_usable(&mut self) -> Task<Message> {
        let ready = !self.demo
            && self.showing_setup
            && self.download.as_ref().is_some_and(|state| {
                state.finished()
                    && !state.daemon_started
                    && !state.starting_daemon
                    && state.start_error.is_none()
            });

        if ready {
            self.start_setup_daemon()
        } else {
            Task::none()
        }
    }

    /// Whether an install is missing a model it needs.
    ///
    /// Reachable only by stopping setup: `setup::needed` sends a half-installed
    /// machine back through it, so the one way to sit here is to have said no.
    /// That makes this a deferred choice rather than a fault - the window says
    /// what is missing and offers to finish, and nothing starts in the
    /// meantime.
    fn incomplete(&self) -> bool {
        !self.models.iter().all(|model| model.installed)
    }

    /// Setup is over: the window becomes the console it was standing in for.
    fn leave_setup(&mut self) {
        self.showing_setup = false;
        self.fading = None;
        self.download = None;
        self.models = system::models();
        self.input = system::default_input();
        self.entries = history::recent();
        self.autostart = system::autostart_enabled();
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        // The socket is the one thing the demo cannot talk over: it is either
        // absent or belongs to a real daemon, and either way its first event
        // would replace the state being looked at.
        if self.demo && matches!(message, Message::Daemon(_)) {
            return Task::none();
        }

        match message {
            Message::Select(section) => {
                self.section = section;
            }
            Message::Tick(now) => {
                // The gap since the last frame, which is what the bar's easing
                // needs: a fraction-per-frame chase would settle at a different
                // speed on a 60Hz screen than on a 144Hz one.
                //
                // Capped, because `self.now` only moves on a frame and frames
                // are only asked for while something is animating. A compositor
                // hitch, or the first tick after the window maps, can be much
                // longer than a frame; a chase given that as one step arrives
                // inside it, which is the snap the speed limit exists to prevent.
                let elapsed = now
                    .saturating_duration_since(self.now)
                    .as_secs_f32()
                    .min(FRAME_CAP);
                self.now = now;
                if let Some(state) = self.download.as_mut() {
                    state.advance(elapsed);
                }
                if let Some(fading) = self.fading.as_mut() {
                    *fading += elapsed;
                    if *fading >= setup::FADE {
                        self.leave_setup();
                    }
                }
                return Task::batch([self.launch_install(), self.setup_usable()]);
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
                self.toggled_at
                    .insert("push_to_talk", std::time::Instant::now());
                self.persist();
            }
            Message::SetCleanup(level) => {
                self.settings.cleanup = level;
                self.toggled_at.insert("cleanup", std::time::Instant::now());
                self.persist();
            }
            Message::Terminal(on) => {
                self.settings.terminal = on;
                self.toggled_at
                    .insert("terminal", std::time::Instant::now());
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
                self.toggled_at
                    .insert("autostart", std::time::Instant::now());
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
            Message::Daemon(daemon::Event::Disconnected) => {
                if believe_disconnect(self.daemon.activity) {
                    self.daemon = daemon::State::default();
                }
            }
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
            Message::ResetChord => {
                self.settings.hotkey = settings::DEFAULT_HOTKEY.to_string();
                self.chord_error = None;
                self.persist();
            }
            Message::CancelCapture => {
                self.capturing = false;
                self.cancel_capture
                    .store(true, std::sync::atomic::Ordering::Relaxed);
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
                if verb == "start" {
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
                        if verb == "start" {
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
            Message::RerunSetup => {
                // Stop first: the daemon holds both models open, and the point
                // of this is to watch setup fetch them rather than to leave a
                // process running on files that no longer exist.
                let _ = system::service("stop");
                match system::remove_models() {
                    Ok(()) => {
                        self.models = system::models();
                        self.daemon = daemon::State::default();
                        return Task::done(Message::BeginSetup);
                    }
                    Err(err) => self.service_error = Some(err),
                }
            }
            Message::BeginSetup => {
                // Demo mode never spawns the installer: the point of it is to
                // lay this screen out on a machine that has no `flow` binary
                // at all, and a failure line is not the state worth looking at.
                if self.demo {
                    self.download = Some(demo::setup_state());
                    self.showing_setup = true;
                    return Task::none();
                }

                // Already on setup (a failed fetch, Try again): the veil is
                // up, so skip the intro and start fetching.
                let skip = self.showing_setup && self.download.is_some();
                let mut state = setup::State::new(setup::Handle::default());
                if skip {
                    state.skip_intro();
                }
                self.download = Some(state);
                self.showing_setup = true;
                return Task::none();
            }
            Message::StopDownload => {
                if let Some(state) = self.download.as_mut() {
                    state.stopped = true;
                    state.handle.stop();
                    // Stopped before the installer was even spawned, so no
                    // `Failed` line is coming to carry the handover. Nothing was
                    // fetched this run either - and a part file here belongs to
                    // an earlier one, which is exactly what a resume is for.
                    if !state.spawned {
                        self.leave_setup();
                    }
                }
            }
            Message::SetupEvent(event) => {
                if let Some(state) = self.download.as_mut() {
                    state.apply(event);
                }

                let over = !self
                    .download
                    .as_ref()
                    .is_some_and(setup::State::downloading);
                let stopped = over && self.download.as_ref().is_some_and(|state| state.stopped);

                // What was downloaded stays downloaded. Stopping used to delete
                // the part file, on the reasoning that bytes of a model someone
                // had decided against would sit there with nothing on screen
                // ever mentioning them - but neither half of that is true any
                // more. Flow needs both models, so there is no deciding against
                // one; and Overview carries a banner saying setup is unfinished
                // with the button that finishes it. The bytes are accounted for.
                //
                // What is left is a 2.4 GB download where Stop threw away
                // everything already fetched. `curl -C -` resumes, so keeping
                // the file makes Stop mean "not now" instead of "start again".
                //
                // It still hands the window over rather than holding them on a
                // ring that failed: the console opens, incomplete, saying what
                // is missing and offering to finish. Treating a stop as a
                // failure would put a Try again in front of the one person who
                // has already said no.
                if stopped {
                    self.leave_setup();
                    return Task::none();
                }

                // Setup keeps its state until it has faded out; anything else
                // is done with the moment it stops, and what is on disk has
                // just changed.
                if over && !self.showing_setup {
                    self.download = None;
                    self.models = system::models();
                }

                return self.setup_usable();
            }
            Message::SetupStarted(result) => {
                let started = result.is_ok();
                if let Some(state) = self.download.as_mut() {
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
                    // What the veil is about to reveal has to be true before it
                    // starts moving, not after. `models` was refreshed only in
                    // `leave_setup`, which runs when the fade ends - so Overview
                    // spent the whole dissolve showing the "setup isn't
                    // finished" banner for the setup that had just finished,
                    // then dropped it as the veil landed.
                    self.models = system::models();
                    // Setup's whole job is done, so it dissolves rather than
                    // waiting to be dismissed. There is nothing on it to read.
                    self.fading = Some(0.0);
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
            (None, true) => text("Saved. Applies to your next dictation.")
                .size(12)
                .color(FAINT),
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
        // The rail/pane divider is its own element: a container border applies
        // to all four sides, and only this edge should be drawn.
        let console = row![self.rail(), vertical_hairline(), self.pane()];

        let Some(state) = self.download.as_ref().filter(|_| self.showing_setup) else {
            return console.into();
        };

        // Setup dissolves into the console rather than being replaced by it.
        // The console sits underneath so the veil has something to fade over -
        // to be looked at, and nothing else. `inert` is what stops the pointer
        // reaching a page that is behind a full-screen overlay.
        let fade = match self.fading {
            Some(elapsed) => 1.0 - (elapsed / setup::FADE).clamp(0.0, 1.0),
            None => 1.0,
        };
        stack![inert(console), setup::view(state, fade)].into()
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
            let enabled = !self.incomplete() || section.works_without_models();
            items = items.push(nav(section, selected, warmth, enabled));
        }

        container(
            column![
                items,
                Space::new().height(Fill),
                // A debug build says so, and says when it was made. See
                // `update::dev_note`.
                container(column![
                    text(update::running())
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                    text(update::dev_note().unwrap_or_default())
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                ])
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
        // A disabled nav item cannot be clicked, but `FLOW_SECTION` can open
        // one directly and a section can be disabled while it is already open -
        // stopping setup from Style would leave it on screen with a nav that
        // says it is unavailable. Overview is where the way out is.
        let section = if self.incomplete() && !self.section.works_without_models() {
            Section::Overview
        } else {
            self.section
        };

        let content = match section {
            Section::Overview => self.overview_section(),
            Section::History => self.history_section(),
            Section::Dictation => self.dictation_section(),
            Section::Audio => self.audio_section(),
            Section::Vocabulary => self.vocabulary_section(),
            Section::Style => self.style_section(),
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
        let left = if matches!(self.section, Section::History | Section::Style) {
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
        // Three states, and the first one rules out the other two:
        //
        //   Not running · needs setup   (no action - the banner owns the way out)
        //   Running                     [Stop]
        //   Not running                 [Start]
        //
        // An unfinished setup is reported as its own state rather than as the
        // daemon's, because the daemon's answer cannot be trusted there: a
        // process left over from before the models went missing still answers
        // the socket, and "Flow is running" over a half-installed product is
        // the one sentence this page must never say.
        let status = status_of(self.incomplete(), self.daemon.activity);
        let needs_setup = status == Status::NeedsSetup;
        let running = status == Status::Running;
        let installed = self.models.iter().filter(|m| m.installed).count();
        // Red, not grey. Grey reads as a resting state, and this is not one:
        // the product cannot dictate until it is dealt with. It is the same
        // severity as the missing-model notes below it, and the two saying it
        // in different colours would make one of them look optional.
        let (label, dot) = if needs_setup {
            ("Not running, needs setup", ERR)
        } else {
            activity_label(self.daemon.activity)
        };
        let verb = if running { "stop" } else { "start" };
        // Always in the row, even while systemd is mid-verb. Pulling it out
        // used to let the Fill space eat its width, so the status walked right
        // on every start and stop. Disabled is how a second press is refused.
        //
        // Gone entirely while setup is unfinished, though, rather than greyed:
        // a disabled Start still says that starting is the answer and that
        // something is stopping you. Nothing is - the models are not there, and
        // the only move is Finish setup in the banner below. Two controls
        // offering the way forward is one control too many.
        let mut header = row![
            text("Overview").size(22).color(FG),
            Space::new().width(Fill),
            pip(dot),
            Space::new().width(9),
            text(label)
                .size(13)
                .color(dot)
                .wrapping(text::Wrapping::None),
        ]
        .align_y(iced::Center);

        if !needs_setup {
            header = header.push(Space::new().width(16));
            header = header.push(action_msg(
                service_action_label(running),
                !running,
                self.service_pending
                    .is_none()
                    .then_some(Message::Service(verb)),
            ));
        }

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
                    (
                        format!("{} average", history::duration(spoken / dictations as f32)),
                        MUTED,
                    )
                },
            ),
            Space::new().width(GAP),
            stat_tile(
                "Current streak",
                plural(streak as u32, "day"),
                (
                    format!(
                        "longest {}",
                        plural(longest_streak(&self.days) as u32, "day")
                    ),
                    MUTED,
                ),
            ),
        ];

        // Sized to fit the default window, so the common case does not scroll
        // at all. Scrolled rather than compressed when it does not fit: a user
        // who drags the window down to the minimum height should have to reach
        // the last card rather than have every card shrink to meet them. The
        // heading is in the scroll, same as every other page: a window this
        // short has to give the cards the room, not keep a title parked over
        // them.
        let mut page = column![];
        // Above everything, because an unfinished install is not a fact about
        // this page - it is a fact about the product, and the way out of it is
        // the only thing on screen worth pressing.
        if let Some(banner) = self.finish_setup_banner() {
            page = page.push(banner);
            page = page.push(Space::new().height(GAP));
        }

        scroll(page.push(column![
            header,
            Space::new().height(SCROLL_PAD),
            self.setup_card(installed),
            Space::new().height(GAP),
            kpis,
            Space::new().height(GAP),
            calendar_card(&self.days),
        ]))
    }

    /// The way out of a stopped setup.
    ///
    /// Only reachable by having pressed Stop, so it is worded as the resumption
    /// of something started rather than as an offer of something extra - and it
    /// does not say what the download costs, because that was on screen when
    /// the decision was made.
    fn finish_setup_banner(&self) -> Option<Element<'_, Message>> {
        if !self.incomplete() {
            return None;
        }

        Some(
            container(
                row![
                    text("Setup isn't finished, so Flow isn't running yet.")
                        .size(12.5)
                        .color(ACCENT),
                    Space::new().width(Fill),
                    action_msg("Finish setup", true, Message::BeginSetup),
                ]
                .align_y(iced::Center),
            )
            .padding([10, 12])
            .width(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(mix(BG, ACCENT, 0.055))),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: mix(BG, ACCENT, 0.22),
                },
                ..Default::default()
            })
            .into(),
        )
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
                // The verb leads. "super shift d to hold" reads as a chord
                // named after an action; "hold super shift d" is the
                // instruction it was meant to be.
                format!(
                    "{} {}",
                    if self.settings.push_to_talk {
                        "hold"
                    } else {
                        "tap"
                    },
                    self.settings.hotkey.replace('+', " "),
                ),
            ),
            Space::new().width(44),
            fact(
                "Microphone",
                clip(self.input.as_deref().unwrap_or("system default"), 38,),
            ),
            Space::new().width(Fill),
            fact("Models", format!("{installed} of {}", self.models.len())),
        ];

        let mut body = column![];
        let notes = self.attention();
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
        let when = latest
            .map(|entry| history::ago(entry.at, history::now()))
            .unwrap_or_default();

        let heading = row![
            text("Last dictation").size(11).color(FAINT),
            Space::new().width(Fill),
            // `ago` already says "just now" for the last minute; empty means
            // the timestamp is missing or in the future, and no label is
            // better than a confident wrong one.
            text(when).size(11).font(Font::MONOSPACE).color(FAINT),
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
            None => text("nothing yet - hold the chord and say something")
                .size(12.5)
                .color(FAINT),
        };

        column![heading, Space::new().height(5), line].into()
    }

    /// Everything on this page that is worth doing something about, in the
    /// order it would bite. Empty when there is nothing to do, and the status
    /// card then collapses to one line - a dashboard that always has a row of
    /// warnings in it teaches people not to read the warnings.
    fn attention(&self) -> Vec<(Color, String)> {
        let mut notes = Vec::new();
        if let Some(problem) = &self.service_error {
            notes.push((ERR, problem.clone()));
        }
        if let Some(problem) = &self.daemon.problem {
            notes.push((ERR, problem.clone()));
        }
        // A missing model is not listed here. The banner above this card
        // already says setup is unfinished and carries the button that fixes
        // it, and the Models fact below counts what arrived - a third line
        // saying the same thing only splits one answer into three.
        if let update::Status::Available(tag) = &self.update {
            notes.push((ACCENT, format!("{tag} is available to install.")));
        }
        notes
    }

    fn history_section(&self) -> Element<'_, Message> {
        let now = history::now();

        let list: Element<'_, Message> = if self.entries.is_empty() {
            // Sits on the heading's left edge and at a row's own top pad, so
            // the line reads as the first entry's place rather than as loose
            // text floating outside the list.
            container(
                text("Nothing yet. Hold the chord and say something.")
                    .size(13)
                    .color(FAINT),
            )
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
                "Hold to talk",
                "On, hold the chord while you speak. Off, tap to start and tap to stop.",
                toggle(
                    self.settings.push_to_talk,
                    self.travel("push_to_talk"),
                    Message::PushToTalk,
                ),
            ),
            setting(
                "Chord",
                // This did once need a restart, and the note saying so outlived
                // the reason: the chord is shared with the config watcher now
                // and `hotkey::spawn` compares it on every key, so a rebinding
                // is live by the next press. The stale line was sending people
                // off to restart for nothing.
                "The keys that start a dictation.",
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
                "Terminal paste chord",
                "Send Ctrl+Shift+V when a terminal has focus.",
                toggle(
                    self.settings.terminal,
                    self.travel("terminal"),
                    Message::Terminal,
                ),
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
                text(
                    self.input
                        .clone()
                        .unwrap_or_else(|| "not detected".to_string()),
                )
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
                toggle(
                    self.settings.denoise,
                    self.travel("denoise"),
                    Message::Denoise,
                ),
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
                    border: Border {
                        color: LINE,
                        width: 1.0,
                        radius: 6.0.into()
                    },
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

    /// How much Flow is allowed to change what you said.
    ///
    /// Four rows rather than a slider or a dropdown: the levels differ by what
    /// they are permitted to touch, which is a difference you can only judge by
    /// reading an example of each. A dropdown shows one option at a time and
    /// makes you remember the rest.
    ///
    /// The example under each title is the same sentence at every level, so the
    /// page demonstrates the difference instead of asserting it.
    fn style_section(&self) -> Element<'_, Message> {
        // Cards, not settings rows: each already carries its own border and
        // tint, so a hairline between them would double the line. A small
        // gap does the separating instead, same as History's entries.
        let mut list = column![];
        for (index, level) in settings::Cleanup::ALL.into_iter().enumerate() {
            if index > 0 {
                list = list.push(Space::new().height(4));
            }
            list = list.push(self.cleanup_row(level));
        }

        scroll_inset(
            column![
                container(heading(
                    "Style",
                    "How much Flow may change what you said. It never leaves this machine.",
                ))
                .padding([0.0, ENTRY_INSET])
                .width(Fill),
                list,
            ],
            PAGE_TOP,
            CONTENT_RIGHT - ENTRY_INSET,
        )
    }

    /// One selectable level. The whole row is the target, because a row with a
    /// radio at one end trains you to aim at the radio.
    fn cleanup_row(&self, level: settings::Cleanup) -> Element<'_, Message> {
        let (title, blurb) = level.describe();
        let chosen = self.settings.cleanup == level;

        // Struck through at None, because that row's example is the one thing
        // on the page that is not an improvement - it is what you actually said.
        let example = text(level.example())
            .size(12)
            .font(Font::MONOSPACE)
            .color(if chosen { MUTED } else { FAINT });

        let body = column![
            row![
                text(title)
                    .size(13.5)
                    .color(if chosen { FG } else { MUTED }),
                Space::new().width(Fill),
                pip(if chosen { OK } else { mix(BG, FG, 0.18) }),
            ]
            .align_y(iced::Center),
            Space::new().height(LABEL_GAP),
            text(blurb).size(12).color(MUTED),
            Space::new().height(6),
            example,
        ];

        let tint = if chosen {
            mix(BG, OK, 0.05)
        } else {
            Color::TRANSPARENT
        };
        iced::widget::button(container(body).padding([10, 12]).width(Fill))
            .padding(0)
            .on_press(Message::SetCleanup(level))
            .style(move |_, status| iced::widget::button::Style {
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered if !chosen => mix(BG, FG, 0.04),
                    _ => tint,
                })),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: if chosen {
                        mix(BG, OK, 0.3)
                    } else {
                        Color::TRANSPARENT
                    },
                },
                ..Default::default()
            })
            .into()
    }

    fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        // Which engines these are is a fact about the build, not a choice -
        // the same class of thing as the version. They stopped being a screen
        // of their own when the install began fetching both.
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
            fact_row("Speech", self.model_fact(0)),
            fact_row("Cleanup", self.model_fact(1)),
            fact_row("Session", self.session.clone()),
            fact_path("Config", &settings::config_path()),
            fact_path("History", &history::path()),
            // The way back to a clean install. Here rather than on Overview
            // because it belongs with the two model rows above it - it is the
            // thing you do when one of them is wrong.
            setting(
                "Run setup again",
                "Deletes both models and fetches them from scratch. About 3 GB.",
                action_msg("Run setup", false, Message::RerunSetup),
            ),
        ];

        section_shell(
            "Flow",
            "Push-to-talk dictation that runs entirely on your own machine.",
            rows,
            None,
        )
    }

    fn model_fact(&self, index: usize) -> String {
        self.models
            .get(index)
            .map(system::Model::fact)
            .unwrap_or_else(|| "unknown".into())
    }

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
                    Space::new().height(LABEL_GAP),
                    text(note).size(12).color(FAINT),
                ],
                Space::new().width(Fill),
                pip(dot),
                Space::new().width(7),
                text(update::running())
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(MUTED),
                Space::new().width(12),
                action,
            ]
            .align_y(iced::Center),
        )
        .padding([ROW_PAD, 0.0])
        .into()
    }
}

/// "No socket yet" is what a start looks like from the outside: systemd has
/// forked the process, but Ready is only published after the models load and
/// the mic warms. Believing that gap used to flash "isn't running" for a
/// second in the middle of Starting.
fn believe_disconnect(activity: daemon::Activity) -> bool {
    activity != daemon::Activity::Starting
}

/// The line and dot colour for each activity the daemon can report. Offline
/// and Ready both read as calm (no accent) - the accent is reserved for the
/// two states where Flow is actually doing something with your voice.
fn activity_label(activity: daemon::Activity) -> (&'static str, Color) {
    match activity {
        daemon::Activity::Offline => ("Flow isn't running", FAINT),
        daemon::Activity::Starting => ("Starting…", STARTING),
        // Ready means the daemon is up and waiting for the hotkey, which is
        // the state a person calls running - and a grey dot beside it read as
        // "nothing is happening" rather than "everything is fine". Green here
        // and green while listening are the same claim at two volumes: Flow is
        // alive. The word beside it is what separates idle from live.
        daemon::Activity::Ready => ("Flow is running", OK),
        daemon::Activity::Listening => ("Listening", ACCENT),
        daemon::Activity::Working => ("Refining your words", ACCENT),
    }
}

/// The one thing to do to the service, which is whichever of its two states
/// it is not in. Restart used to live here, and a control that reads the same
/// whether Flow is up or down says nothing about which it is; Start and Stop
/// are the status told twice, once as a word and once as an offer.
fn service_action_label(running: bool) -> &'static str {
    if running {
        "Stop"
    } else {
        "Start"
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

/// The three states the Overview can report, and the only three there are.
///
/// `NeedsSetup` wins over whatever the socket says. A daemon left running from
/// before the models went missing still answers it, and reporting that as
/// Running would put "Flow is running" over a product that cannot dictate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    /// Setup unfinished. No start, no stop - the banner owns the way out.
    NeedsSetup,
    /// Up, and can be stopped.
    Running,
    /// Down, and can be started.
    Stopped,
}

fn status_of(incomplete: bool, activity: daemon::Activity) -> Status {
    if incomplete {
        Status::NeedsSetup
    } else if activity == daemon::Activity::Offline {
        Status::Stopped
    } else {
        Status::Running
    }
}

/// Read the keyboard until a chord arrives, on whatever thread the runtime
/// gives us. Split out so the async block above stays a one-liner.
fn tokio_free_capture(cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<String> {
    chord::capture(&|| cancelled.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::{
        activity_label, believe_disconnect, service_action_label, status_of, Section, Status,
    };
    use crate::daemon;
    use crate::theme::STARTING;

    /// Running and Stopped are states of a product that is installed. Neither
    /// may be reported while a model is missing - a daemon left over from
    /// before that happened still answers the socket, and believing it would
    /// put "Flow is running" over something that cannot dictate a word.
    #[test]
    fn an_unfinished_setup_outranks_whatever_the_socket_says() {
        for activity in [
            daemon::Activity::Offline,
            daemon::Activity::Starting,
            daemon::Activity::Ready,
            daemon::Activity::Listening,
            daemon::Activity::Working,
        ] {
            assert_eq!(
                status_of(true, activity),
                Status::NeedsSetup,
                "{activity:?} was reported as something other than needing setup"
            );
        }
    }

    #[test]
    fn a_finished_setup_reports_the_daemon() {
        assert_eq!(status_of(false, daemon::Activity::Offline), Status::Stopped);
        assert_eq!(status_of(false, daemon::Activity::Ready), Status::Running);
        assert_eq!(status_of(false, daemon::Activity::Working), Status::Running);
    }

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

    #[test]
    fn startup_is_named_once_in_the_status() {
        assert_eq!(
            activity_label(daemon::Activity::Starting),
            ("Starting…", STARTING)
        );
        assert_eq!(service_action_label(false), "Start");
        // The action is the state it is not in - never a word that reads the
        // same either way.
        assert_eq!(service_action_label(true), "Stop");
    }

    #[test]
    fn a_missing_socket_during_startup_is_not_offline() {
        assert!(!believe_disconnect(daemon::Activity::Starting));
        assert!(believe_disconnect(daemon::Activity::Ready));
        assert!(believe_disconnect(daemon::Activity::Offline));
        assert!(believe_disconnect(daemon::Activity::Listening));
    }
}
