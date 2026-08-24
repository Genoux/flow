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
mod layout;
mod screen;
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
    entry_list, entry_row, fact_path, fact_row, group, heading, inert, nav, page_shell, scroll,
    scroll_inset, section_shell, setting,
};
use crate::theme::{
    mix, progress, ACCENT, BG, CALENDAR_DAYS, CONTENT_RIGHT, COPIED, ENTRY_INSET, ERR, FADE, FAINT,
    FG, GAP, KNOB, LABEL_GAP, LINE, MUTED, OK, PAGE_TOP, PANE_INSET, RAIL_WIDTH, ROW_PAD,
    SCROLL_PAD, STARTING, WARN,
};
use iced::{Color, Subscription, Task, Theme};

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
    /// Both models are required, so a machine missing either has no daemon at
    /// all - not a daemon doing less. That makes every screen that tunes
    /// dictation a screen tuning nothing: Settings, Vocabulary and Style
    /// all describe behaviour that has no process to belong to.
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
            Section::Vocabulary => "Vocabulary",
            Section::Style => "Style",
            Section::Settings => "Settings",
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
    Denoise(bool),
    Sound(bool),
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
    /// How many installed files are missing or the wrong length, asked of the
    /// daemon binary at launch and again whenever setup ends. `None` when
    /// nothing could answer.
    damage: Option<usize>,
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

        // A machine with no speech model cannot dictate, so the window has
        // nothing to report and one thing to do.
        let first_run = setup::needed();

        let settings = settings::Settings::load();

        (
            Self {
                section: Section::initial(),
                daemon: daemon::State::default(),
                settings,
                save_error: None,
                service_error: None,
                service_pending: None,
                autostart: system::autostart_enabled(),
                input: system::default_input(),
                entries,
                copied: None,
                days: history::daily(CALENDAR_DAYS),
                // Already checking, because it is: the check goes out with
                // this window. Left at Unknown, About would read "not checked
                // yet" while a check was in flight and its button would fire a
                // second one.
                update: update::Status::Checking,
                updating: false,
                models: system::models(),
                damage: system::damage(),
                download: None,
                showing_setup: first_run,
                fading: None,
                session: system::session(),
                terms: vocabulary::load(),
                typing: String::new(),
                term_error: None,
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
                Task::perform(async { update::latest() }, Message::UpdateChecked),
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
        if state.spawned || state.stopped || !state.intro_over() {
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
        let ready = self.showing_setup
            && self.download.as_ref().is_some_and(|state| {
                state.finished()
                    && !state.daemon_started
                    && !state.starting_daemon
                    && state.start_error.is_none()
            });

        if !ready {
            return Task::none();
        }

        // Nothing came down the wire, so nothing on disk changed and the daemon
        // is already running the files a restart would hand it. Restarting it
        // to prove a repair found nothing wrong is a dropped socket and a model
        // reloaded for no one. It only has to start if it is not running.
        let repaired_nothing = self.download.as_ref().is_some_and(|state| !state.fetching)
            && self.daemon.activity != daemon::Activity::Offline;

        if repaired_nothing {
            if let Some(state) = self.download.as_mut() {
                state.daemon_started = true;
            }
            self.models = system::models();
            self.fading = Some(-setup::HOLD);
            return Task::none();
        }

        self.start_setup_daemon()
    }

    /// Whether an install is missing a model it needs.
    ///
    /// Reachable only by stopping setup: `setup::needed` sends a half-installed
    /// machine back through it, so the one way to sit here is to have said no.
    /// That makes this a deferred choice rather than a fault - the window says
    /// what is missing and offers to finish, and nothing starts in the
    /// meantime.
    fn incomplete(&self) -> bool {
        match self.damage {
            Some(count) => count > 0,
            // No verdict. What the window can see for itself is whether the
            // models are there at all, which is what it used to go on.
            None => !self.models.iter().all(|model| model.installed),
        }
    }

    /// Why the Overview banner is up, if it is.
    ///
    /// One banner, two situations, and they are not the same news. A machine
    /// with nothing on it has not finished setting up - green, an invitation.
    /// A machine that had both models and lost a file out of one is broken -
    /// amber, and saying "setup isn't finished" to someone who finished it a
    /// month ago is how a real fault gets read as a glitch.
    fn install_problem(&self) -> Option<InstallProblem> {
        if !self.incomplete() {
            return None;
        }
        Some(if self.models.iter().any(|model| model.installed) {
            InstallProblem::Damaged
        } else {
            InstallProblem::Unfinished
        })
    }

    /// Setup is over: the window becomes the console it was standing in for.
    fn leave_setup(&mut self) {
        self.showing_setup = false;
        self.fading = None;
        self.download = None;
        self.models = system::models();
        self.damage = system::damage();
        self.input = system::default_input();
        self.entries = history::recent();
        self.autostart = system::autostart_enabled();
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
        daemon::Activity::Offline => ("Not running", FAINT),
        daemon::Activity::Starting => ("Starting…", STARTING),
        // Ready means the daemon is up and waiting for the hotkey, which is
        // the state a person calls running - and a grey dot beside it read as
        // "nothing is happening" rather than "everything is fine". Green here
        // and green while listening are the same claim at two volumes: Flow is
        // alive. The word beside it is what separates idle from live.
        daemon::Activity::Ready => ("Running", OK),
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
    /// Nothing installed: setup was never finished, or was stopped.
    Unfinished,
    /// Installed, then a file went missing or came back the wrong length.
    Damaged,
}

impl InstallProblem {
    /// The line, the button, and the colour the banner is drawn in.
    fn banner(self) -> (&'static str, &'static str, Color) {
        match self {
            Self::Unfinished => ("Setup isn't finished.", "Finish setup", ACCENT),
            Self::Damaged => ("A file is missing or damaged.", "Repair", WARN),
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

#[cfg(test)]
mod install_banner {
    use super::*;

    #[test]
    fn a_lost_file_warns_in_amber_rather_than_inviting_in_green() {
        let (line, offer, tone) = InstallProblem::Damaged.banner();
        assert_eq!(tone, WARN, "a fault must not wear the invitation's colour");
        assert_ne!(tone, ACCENT);
        assert!(line.contains("damaged"), "{line}");
        assert_eq!(offer, "Repair");
    }

    #[test]
    fn an_unfinished_setup_stays_an_invitation() {
        let (line, offer, tone) = InstallProblem::Unfinished.banner();
        assert_eq!(tone, ACCENT);
        assert!(line.contains("Setup isn't finished"), "{line}");
        assert_eq!(offer, "Finish setup");
    }
}
