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
mod dispatch;
mod format;
mod history;
mod interaction;
mod layout;
mod motion;
mod screen;
mod settings;
mod smooth_scroll;
mod storage;
mod system;
mod theme;
mod update;
mod vocabulary;

use crate::calendar::{calendar_card, current_streak, longest_streak};
use crate::card::stat_tile;
use crate::control::{action_msg, card_rule, pip, toggle, value_slider, vertical_hairline};
use crate::format::{commas, plural, trend};
use crate::layout::{
    entry_list, entry_row, fact_path, fact_row, group, heading, inert, nav, page_shell, scroll,
    scroll_inset, section_shell, setting,
};
use crate::theme::{
    mix, progress, ACCENT, BG, CALENDAR_DAYS, CARD_RADIUS, CONTENT_RIGHT, COPIED, ENTRY_INSET, ERR,
    FADE, FAINT, FG, GAP, HAIRLINE, LABEL_GAP, MUTED, OK, PAGE_TOP, PANE_INSET, RAIL_WIDTH,
    ROW_PAD, STARTING,
};
use iced::{Color, Subscription, Task, Theme};

fn main() -> iced::Result {
    iced::application(Console::new, Console::update, Console::view)
        .title("Flow")
        .antialiasing(true)
        .font(include_bytes!("../../../assets/NotoSerifDisplay-Regular.ttf").as_slice())
        .theme(theme)
        .subscription(subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(1060.0, 694.0),
            exit_on_close_request: false,
            position: iced::window::Position::Centered,
            // Not scaled with the opening size, and deliberately: this is not a
            // matter of taste like the line above it, it is the point below
            // which the layout stops working - the rail is a fixed 176 and the
            // pane's insets are fixed either side of it, so the floor is set by
            // what still fits rather than by how big the window should feel.
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
    let mut subs = vec![
        daemon,
        iced::window::close_requests().map(Message::CloseRequested),
    ];

    // Escape shuts the dialog. A modal you can only leave with the pointer is
    // a modal somebody gets stuck in, and the listener costs nothing while
    // there is no dialog to shut.
    if matches!(state.picking_input, Some(Picker::Opening(_))) {
        subs.push(iced::event::listen_with(|event, _status, _window| {
            matches!(
                event,
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                })
            )
            .then_some(Message::ClosePicker)
        }));
    }

    // Redrawing every frame forever to animate nothing would be a way to make
    // a settings window cost battery.
    if state.moving() {
        subs.push(iced::window::frames().map(Message::Tick));
    }
    Subscription::batch(subs)
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
    Vocabulary,
    /// What used to be Models. Both models now arrive with the install, so the
    /// screen that asked which to fetch has no question left on it - what it
    /// has instead is the one choice that changes what Flow writes.
    Style,
    /// Dictation and Audio merged. Eight rows across two rail sections meant a
    /// click to discover which one held the switch you wanted; they are labelled
    /// groups on one page now.
    ///
    /// Last but for About, because it is where you go to change something rather
    /// than where you start - the screens above it are the ones with your words
    /// on them.
    Settings,
    About,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::Overview,
        Section::History,
        Section::Vocabulary,
        Section::Style,
        Section::Settings,
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
    /// Without a key there is no daemon at all - not a daemon doing less - so
    /// Vocabulary and Style describe behaviour with no process to belong to.
    ///
    /// Settings is the exception it did not used to be: it is where the key
    /// goes, so disabling it would lock the one door out of this state.
    /// Overview survives because it says what is wrong, and About because a
    /// version and a path are true whether or not anything is running.
    ///
    /// Disabled rather than hidden. A nav that grows items once a key is
    /// pasted is a nav that was lying about what the product is.
    fn works_without_models(self) -> bool {
        matches!(self, Section::Overview | Section::About | Section::Settings)
    }

    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::History => "History",
            Section::Vocabulary => "Vocabulary",
            Section::Style => "Style",
            Section::Settings => "Settings",
            Section::About => "About",
        }
    }
}

/// The microphone dialog's clock, and which way it is running.
///
/// Not a bool, and that is the whole point: a dismissed dialog has to stay in
/// the widget tree until its fade has run out, so `Closing` is a dialog you can
/// still see. It used to be `Option<Instant>` cleared on close, which meant the
/// thing faded in over 200ms and then vanished between two frames - the arrival
/// was animated and the departure was a cut.
#[derive(Debug, Clone, Copy)]
enum Picker {
    Opening(std::time::Instant),
    Closing(std::time::Instant),
}

impl Picker {
    /// 1 when fully open, 0 when it is no longer there.
    ///
    /// Eased on the way in and linear on the way out, which is not a lapse.
    /// `progress` is a quartic ease-out - the right curve for an arrival, since
    /// it settles rather than stops - and the exit was written as `1 - progress`
    /// on the assumption that reversing a good curve gives a good curve. It does
    /// not: inverted, the quartic front-loads the whole fade, so the dialog was
    /// under 7% opacity a quarter of the way through 200ms and then hung there
    /// as a ghost for the rest. Opacity has no position to settle into, so an
    /// exit has nothing to gain from easing and everything to lose from a tail.
    fn lift(self, now: std::time::Instant) -> f32 {
        match self {
            Self::Opening(at) => progress(at, now, FADE),
            Self::Closing(at) => {
                let elapsed = now.saturating_duration_since(at).as_millis() as f32;
                1.0 - (elapsed / FADE as f32).clamp(0.0, 1.0)
            }
        }
    }

    /// Faded out and finished with. `view` stops drawing it; `Tick` drops it.
    fn spent(self, now: std::time::Instant) -> bool {
        matches!(self, Self::Closing(_)) && self.lift(now) <= 0.0
    }

    fn since(self) -> std::time::Instant {
        match self {
            Self::Opening(at) | Self::Closing(at) => at,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    TypingKey(String),
    /// Switch which build runs on the next restart.
    SetChannel(bool),
    ChannelInstalled(Result<(), String>),
    RestartApp,
    AppRestarted(Result<(), String>),
    SaveKey,
    Select(Section),
    BannersAllocated(Vec<Result<iced::advanced::image::Allocation, iced::advanced::image::Error>>),
    PushToTalk(bool),
    SettingsSaved(Result<(), String>),
    CloseRequested(iced::window::Id),
    /// A cleanup card on the Style screen. Picking a level is the whole of that
    /// screen, so it saves immediately rather than behind a confirm.
    SetCleanup(settings::Cleanup),
    Denoise(bool),
    /// Which microphone to record from. `None` is Auto-detect - follow whatever
    /// the system default is, now and whenever it changes. Picking one also
    /// closes the dialog it was picked in: the choice takes effect on the next
    /// press, so there is nothing left to confirm.
    InputDevice(Option<String>),
    /// Open the microphone dialog. See `mic_dialog`.
    PickInput,
    /// Dismiss it without choosing - the close button, or a click on the veil.
    ClosePicker,
    Sound(bool),
    ShowTray(bool),
    TrayStarted(Result<(), String>),
    Autostart(bool),
    AutostartFinished(Result<Option<bool>, String>),
    PathOpened(Result<(), String>),
    HistoryLoaded(Vec<history::Entry>, Vec<history::Day>),
    Duck(u32),
    OpenConfig,
    /// Reveal a path in the file manager. About's config and history rows.
    OpenPath(std::path::PathBuf),
    /// systemctl --user <verb> flow.service
    Service(&'static str),
    /// A service command finished away from the UI thread.
    ServiceFinished(&'static str, Result<(), String>),
    /// Launch-time reads that may wait on PipeWire or an input device finished
    /// away from the UI thread.
    PeripheralsLoaded(Peripherals),
    /// Start listening for the next chord the user presses.
    CaptureChord,
    /// A key arrived while capturing.
    Captured(Option<String>),
    CancelCapture,
    /// Put the chord back to what a fresh install uses.
    ResetChord,
    TypingTerm(String),
    FilterTerms(String),
    HoverCleanup(Option<settings::Cleanup>),
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
    /// Take the user to Settings, which is where the key goes.
    BeginSetup,
    CheckUpdate,
    UpdateChecked(update::Status),
    InstallUpdate,
    UpdateInstalled(Result<String, String>),
}

/// Machine state that informs controls but does not decide the page's shape.
/// It is deliberately loaded after construction: PipeWire and evdev are
/// external systems, and neither gets to hold the window's first frame.
#[derive(Debug, Clone)]
struct Peripherals {
    input: Option<String>,
    sources: Vec<(String, String)>,
    can_capture: bool,
}

impl Peripherals {
    fn read() -> Self {
        Self {
            input: system::default_input(),
            sources: system::input_sources(),
            can_capture: chord::available(),
        }
    }
}

struct Console {
    section: Section,
    banner_allocations: Vec<iced::advanced::image::Allocation>,
    banners_ready: bool,
    page_motion: motion::PageTransition,
    daemon: daemon::State,
    settings: settings::Settings,
    save_pending: bool,
    save_dirty: bool,
    closing_window: Option<iced::window::Id>,
    /// Set when a save fails, so a read-only config or a full disk is visible
    /// rather than a control that silently springs back.
    save_error: Option<String>,
    /// A service failure belongs on Overview, beside the control that caused
    /// it, instead of in the settings-only save status.
    service_error: Option<String>,
    /// The service verb currently running. Kept separate from daemon activity:
    /// the socket may still report Offline while systemd is starting it.
    service_pending: Option<&'static str>,
    /// None when systemd cannot answer - the control is hidden rather than
    /// shown in a state we cannot vouch for.
    autostart: Option<bool>,
    autostart_pending: bool,
    peripherals_pending: bool,
    history_pending: bool,
    history_dirty: bool,
    /// The description of the system default source, which is what Auto-detect
    /// resolves to.
    input: Option<String>,
    /// Every microphone that can be picked, as (source name, description).
    sources: Vec<(String, String)>,
    /// The microphone dialog while it is on screen or on its way off, `None`
    /// once it is gone. See [`Picker`].
    picking_input: Option<Picker>,
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
    restart_pending: bool,
    /// How many installed files are missing or the wrong length, asked of the
    /// daemon binary at launch and again whenever setup ends. `None` when
    /// Seconds into a full-window transition. Nothing drives it since the
    /// setup screen went, kept because `moving` still asks.
    fading: Option<f32>,
    session: String,
    terms: Vec<String>,
    typing: String,
    term_query: String,
    term_error: Option<String>,
    /// What is in the OpenRouter key box right now, which is not yet what is
    /// saved. Typed keys are persisted on Enter, not per keystroke: a
    /// half-pasted credential written to disk is a config file that fails
    /// authentication until the paste finishes.
    typing_key: String,
    /// Which build the `flow` symlink points at, read at launch. The link is
    /// the source of truth; this is only what the switch renders.
    channel: system::Channel,
    /// True while waiting for the user to press a new chord.
    capturing: bool,
    /// False when /dev/input cannot be read, so the chord cannot be captured.
    can_capture: bool,
    cancel_capture: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Why the last attempted chord was rejected, shown in place of the hint.
    chord_error: Option<String>,

    now: std::time::Instant,
    nav_motion: [motion::Transition; 6],
    cleanup_selection: [motion::Transition; 3],
    cleanup_hover: [motion::Transition; 3],
    entry_motion: std::collections::HashMap<usize, motion::Transition>,
    /// When each toggle last flipped, so its knob can travel rather than jump.
    toggle_motion: std::collections::HashMap<&'static str, motion::Transition>,
}

impl Console {
    fn new() -> (Self, Task<Message>) {
        let entries = history::recent();

        // A machine with no key cannot dictate, so the window has nothing to
        // report and one thing to do: Overview says so, and Settings is where
        // the key goes. There is no longer anything to download, so this is a
        // banner rather than a screen of its own.
        let first_run = settings::Settings::load().openrouter_key.is_none();

        let settings = settings::Settings::load();
        let cleanup_selection = settings::Cleanup::ALL.map(|level| {
            motion::Transition::new(if settings.cleanup == level { 1.0 } else { 0.0 })
        });

        (
            Self {
                section: Section::initial(),
                banner_allocations: Vec::new(),
                banners_ready: false,
                page_motion: motion::PageTransition::default(),
                daemon: daemon::State::default(),
                settings,
                save_pending: false,
                save_dirty: false,
                closing_window: None,
                save_error: None,
                service_error: None,
                service_pending: None,
                autostart: system::startup_autostart_enabled(),
                autostart_pending: false,
                peripherals_pending: true,
                history_pending: false,
                history_dirty: false,
                input: None,
                sources: Vec::new(),
                picking_input: None,
                entries,
                copied: None,
                days: history::daily(CALENDAR_DAYS),
                // Already checking, because it is: the check goes out with
                // this window. Left at Unknown, About would read "not checked
                // yet" while a check was in flight and its button would fire a
                // second one.
                update: update::Status::Checking,
                updating: false,
                restart_pending: false,
                fading: None,
                session: system::session(),
                terms: vocabulary::load(),
                typing: String::new(),
                term_query: String::new(),
                term_error: None,
                typing_key: String::new(),
                channel: system::channel(),
                capturing: false,
                can_capture: false,
                cancel_capture: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                chord_error: None,
                now: std::time::Instant::now(),
                nav_motion: Section::ALL.map(|_| motion::Transition::new(0.0)),
                cleanup_selection,
                cleanup_hover: settings::Cleanup::ALL.map(|_| motion::Transition::new(0.0)),
                entry_motion: std::collections::HashMap::new(),
                toggle_motion: std::collections::HashMap::new(),
            },
            // Setup starts itself. Launching Flow with nothing installed is
            // already the request; a Begin button in front of it would only be
            // asking the same question twice.
            Task::batch([
                screen::preload_images(),
                // PipeWire can stall while devices are being relinked (remote
                // desktop connections do exactly that), and closing even one
                // evdev descriptor can wait on the kernel. Neither answer is
                // needed to draw the window, so let the first frame win.
                Task::perform(async { Peripherals::read() }, Message::PeripheralsLoaded),
                if first_run {
                    Task::done(Message::BeginSetup)
                } else {
                    // Opening an installed Flow is also asking it to be ready.
                    // `start` is harmless when the daemon is already up, and
                    // gives a stopped daemon its expected default behaviour.
                    Task::perform(async { system::service("start") }, |result| {
                        Message::ServiceFinished("start", result)
                    })
                },
                // The tray is independent of dictation. Restore its lightweight
                // service whenever the window opens, even if dictation cannot
                // start or the user has chosen to leave it stopped.
                Task::perform(async { system::start_tray() }, Message::TrayStarted),
                // Whether there is a newer Flow, asked without being asked.
                // A release nobody knows about is a release nobody installs,
                // and the answer belongs on screen before the question occurs
                // to anyone - the Overview names an available version among
                // its notes, so opening the window is enough to hear about it.
                //
                // Quiet when it goes wrong: a check that fails says so on the
                // About screen and nowhere else, so a laptop with no network
                // opens on exactly the window it opened on before.
                Task::perform(async { update::latest() }, Message::UpdateChecked),
            ]),
        )
    }

    fn animate_toggle(&mut self, key: &'static str, previous: bool, target: bool) {
        self.toggle_motion
            .entry(key)
            .or_insert_with(|| motion::Transition::new(u8::from(previous) as f32))
            .set(u8::from(target) as f32, std::time::Instant::now());
    }

    fn travel(&self, key: &str) -> f32 {
        let on = match key {
            "push_to_talk" => self.settings.push_to_talk,
            "denoise" => self.settings.denoise,
            "sound" => self.settings.sound,
            "show_tray" => self.settings.show_tray,
            "autostart" => self.autostart.unwrap_or(false),
            _ => return 1.0,
        };
        let position = self
            .toggle_motion
            .get(key)
            .map(|motion| motion.value(self.now))
            .unwrap_or(u8::from(on) as f32);
        if on {
            position
        } else {
            1.0 - position
        }
    }

    /// True while any motion is still running, which is what decides whether
    /// to ask for frames at all.
    fn moving(&self) -> bool {
        let running = |since: std::time::Instant, ms: u64| {
            self.now.saturating_duration_since(since).as_millis() < ms as u128
        };
        self.nav_motion
            .iter()
            .chain(&self.cleanup_selection)
            .chain(&self.cleanup_hover)
            .any(|motion| motion.moving(self.now))
            || self.page_motion.moving()
            || self
                .entry_motion
                .values()
                .any(|motion| motion.moving(self.now))
            || self.copied.is_some_and(|(_, at)| running(at, COPIED))
            || self
                .toggle_motion
                .values()
                .any(|motion| motion.moving(self.now))
            || self
                .picking_input
                .is_some_and(|picker| running(picker.since(), FADE))
            || self.fading.is_some()
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
        self.entry_motion
            .get(&index)
            .map(|motion| motion.value(self.now))
            .unwrap_or(0.0)
    }

    fn persist(&mut self) -> Task<Message> {
        if self.save_pending {
            self.save_dirty = true;
            return Task::none();
        }
        self.save_pending = true;
        let settings = self.settings.clone();
        Task::perform(
            async move { settings.save().map_err(|error| error.to_string()) },
            Message::SettingsSaved,
        )
    }

    /// Why the Overview banner is up, if it is.
    ///
    /// One situation now, where there used to be two. A missing or damaged
    /// model file was a fault the window could see and offer to repair; there
    /// are no files any more, so the only thing that stops Flow working before
    /// it starts is the absence of a key - and that is a thing to finish, not
    /// a fault to fix.
    fn incomplete(&self) -> bool {
        self.install_problem().is_some()
    }

    fn install_problem(&self) -> Option<InstallProblem> {
        self.settings
            .openrouter_key
            .is_none()
            .then_some(InstallProblem::Unfinished)
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
///
/// Ready is the one that had to change. "Running" was true of the process and
/// silent about the only thing that decides whether the hotkey works: every
/// dictation is an OpenRouter request now, so a daemon that is up with a dead
/// network or a rejected key is running and useless. The word says which.
fn activity_label(activity: daemon::Activity, reachable: Option<bool>) -> (&'static str, Color) {
    match activity {
        daemon::Activity::Offline => ("Not running", FAINT),
        daemon::Activity::Starting => ("Starting…", STARTING),
        // Unknown is not a failure and must not be coloured like one: the
        // daemon is up and has simply not had a dictation to send yet.
        daemon::Activity::Ready => match reachable {
            Some(true) => ("Connected", OK),
            Some(false) => ("Disconnected", ERR),
            None => ("Running", OK),
        },
        daemon::Activity::Listening => ("Listening", ACCENT),
        daemon::Activity::Working => ("Refining", ACCENT),
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
        update::Status::Unknown => (FAINT, "not checked yet".into()),
        update::Status::Checking => (MUTED, "checking…".into()),
        update::Status::Current => (OK, "up to date".into()),
        update::Status::Available(tag) => (ACCENT, format!("{tag} is available")),
        update::Status::Installed(tag) => (OK, format!("{tag} installed - restart Flow")),
        update::Status::Failed(why) => (ERR, format!("could not check: {why}")),
    }
}

/// Why the Overview is showing a banner. See `Console::install_problem`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallProblem {
    /// No key, which is the only way Flow can be unable to work before it
    /// starts. `Damaged` used to sit beside this, for a model file that went
    /// missing; there are no files to lose any more.
    Unfinished,
}

impl InstallProblem {
    /// The line, the button, and the colour the banner is drawn in.
    fn banner(self) -> (&'static str, &'static str, Color) {
        match self {
            Self::Unfinished => ("No OpenRouter key yet.", "Open Settings", ACCENT),
        }
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

    #[test]
    fn closing_waits_for_the_latest_settings_save() {
        use super::{Console, Message};
        let (mut console, _) = Console::new();
        let first = console.update(Message::Duck(10));
        assert!(first.units() > 0);
        assert_eq!(console.update(Message::Duck(20)).units(), 0);
        let window = iced::window::Id::unique();
        assert_eq!(console.update(Message::CloseRequested(window)).units(), 0);
        assert!(console.update(Message::SettingsSaved(Ok(()))).units() > 0);
        assert!(console.save_pending);
        assert_eq!(console.closing_window, Some(window));
        assert_eq!(console.settings.duck, 20);
        assert!(console.update(Message::SettingsSaved(Ok(()))).units() > 0);
        assert!(!console.save_pending);
        assert!(console.closing_window.is_none());
    }

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
        assert_eq!(Section::from_label("  settings "), Some(Section::Settings));
        assert_eq!(Section::from_label("SETTINGS"), Some(Section::Settings));
        assert_eq!(Section::from_label("set"), None);
        assert_eq!(Section::from_label(""), None);
    }

    #[test]
    fn startup_is_named_once_in_the_status() {
        assert_eq!(
            activity_label(daemon::Activity::Starting, None),
            ("Starting…", STARTING)
        );
        assert_eq!(service_action_label(false), "Start");
        // The action is the state it is not in - never a word that reads the
        // same either way.
        assert_eq!(service_action_label(true), "Stop");
    }

    /// "Running" said the process was up. It never said whether a dictation
    /// could reach anything, which after the move to OpenRouter is the whole
    /// question.
    #[test]
    fn ready_reports_the_connection_rather_than_the_process() {
        assert_eq!(
            activity_label(daemon::Activity::Ready, Some(true)).0,
            "Connected"
        );
        assert_eq!(
            activity_label(daemon::Activity::Ready, Some(false)).0,
            "Disconnected"
        );
        // Nothing has been sent yet, so there is nothing to claim either way.
        assert_eq!(activity_label(daemon::Activity::Ready, None).0, "Running");
        // A daemon that is down is down, whatever the network is doing.
        assert_eq!(
            activity_label(daemon::Activity::Offline, Some(true)).0,
            "Not running"
        );
    }

    #[test]
    fn a_missing_socket_during_startup_is_not_offline() {
        assert!(!believe_disconnect(daemon::Activity::Starting));
        assert!(believe_disconnect(daemon::Activity::Ready));
        assert!(believe_disconnect(daemon::Activity::Offline));
        assert!(believe_disconnect(daemon::Activity::Listening));
    }
}

#[cfg(test)]
mod install_banner {
    use super::*;

    #[test]
    fn a_missing_key_reads_as_an_invitation_not_a_fault() {
        let (line, offer, tone) = InstallProblem::Unfinished.banner();
        assert_eq!(tone, ACCENT, "nothing is broken - it is unfinished");
        assert!(line.contains("key"), "{line}");
        assert_eq!(offer, "Open Settings");
    }
}
