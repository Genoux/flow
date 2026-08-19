//! Flow's status and settings window.
//!
//! A separate binary from the daemon on purpose: iced brings wgpu with it, and
//! the daemon has no business carrying that to record audio. The two talk over
//! the status socket the daemon already publishes, and everything else on
//! screen is read from the same files the daemon uses: `settings` edits
//! `config.toml`, `history` reads the transcript log, `vocabulary` edits
//! `vocabulary.txt`.

mod chord;
mod daemon;
mod history;
mod settings;
mod system;
mod update;
mod vocabulary;

use iced::widget::{button, column, container, row, scrollable, slider, text, text_editor, Space};
use iced::{Background, Border, Color, Element, Fill, Font, Length, Subscription, Task, Theme};

// ---------------------------------------------------------------------------
// Tokens. Taken from the island in overlay.rs so the window and the overlay
// read as one product: its ground colour, its restraint, one warm accent that
// only ever means "live".
// ---------------------------------------------------------------------------

const BG: Color = Color { r: 0.039, g: 0.043, b: 0.055, a: 1.0 }; // #0A0B0E
const FG: Color = Color { r: 0.925, g: 0.929, b: 0.937, a: 1.0 }; // #ECEDEF
const MUTED: Color = Color { r: 0.486, g: 0.510, b: 0.549, a: 1.0 }; // #7C828C
const FAINT: Color = Color { r: 0.306, g: 0.329, b: 0.361, a: 1.0 }; // #4E545C
const LINE: Color = Color { r: 0.106, g: 0.118, b: 0.137, a: 1.0 }; // #1B1E23
/// The lifted surface a rail item sits on when selected or hovered. One step
/// off the ground, no more: the rail is chrome and should stay quiet.
const RAISED: Color = Color { r: 0.102, g: 0.110, b: 0.125, a: 1.0 }; // #1A1C20
const ACCENT: Color = Color { r: 0.847, g: 0.651, b: 0.341, a: 1.0 }; // #D8A657
const ERR: Color = Color { r: 0.831, g: 0.451, b: 0.420, a: 1.0 }; // #D4736B
/// The only green in the product, and it means exactly one thing: nothing to
/// do. Muted to the same weight as ERR so a row of dots reads as one family.
const OK: Color = Color { r: 0.549, g: 0.729, b: 0.478, a: 1.0 }; // #8CBA7A
const ON_ACCENT: Color = Color { r: 0.078, g: 0.082, b: 0.059, a: 1.0 };

const RAIL_WIDTH: f32 = 176.0;

/// How far back the Overview activity calendar looks. 26 weeks reads like a
/// half-year at a glance without the grid outgrowing the pane.
const CALENDAR_WEEKS: usize = 26;
const CALENDAR_DAYS: usize = CALENDAR_WEEKS * 7;

/// How long each motion takes. Only two things move - a toggle's knob and a
/// rail item warming under the pointer - because those are the two that
/// acknowledge something the user just did. Long enough that the easing
/// curve is visible rather than read as a snap.
const KNOB: u64 = 220;
const FADE: u64 = 200;

/// Quartic ease-out. Everything here moves fastest at the start and settles
/// gently into place rather than stopping - a steeper tail than cubic, which
/// is what turns "arrives" into "settles".
fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

/// 0.0 at `since`, 1.0 once `ms` has passed.
fn progress(since: std::time::Instant, now: std::time::Instant, ms: u64) -> f32 {
    ease_out(now.saturating_duration_since(since).as_millis() as f32 / ms as f32)
}

/// Blend two colours, for hover states and the settling of a fade.
fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        from.r + (to.r - from.r) * t,
        from.g + (to.g - from.g) * t,
        from.b + (to.b - from.b) * t,
        from.a + (to.a - from.a) * t,
    )
}

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
    Cleanup(bool),
    Terminal(bool),
    Denoise(bool),
    Autostart(bool),
    Duck(u32),
    OpenConfig,
    /// systemctl --user <verb> flow.service
    Service(&'static str),
    /// Start listening for the next chord the user presses.
    CaptureChord,
    /// A key arrived while capturing.
    Captured(Option<String>),
    CancelCapture,
    TypingTerm(String),
    AddTerm,
    RemoveTerm(usize),
    Daemon(daemon::Event),
    /// A frame went by; only delivered while something is moving.
    Tick(std::time::Instant),
    Hover(Option<Section>),
    /// A selection/copy action on one history entry's transcript.
    HistoryAction(usize, text_editor::Action),
    InstallModels,
    ModelsInstalled(Result<(), String>),
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
    /// True once anything has been written. The daemon only reads its config at
    /// startup, so the window has to say so rather than imply a live change.
    saved: bool,
    /// None when systemd cannot answer - the control is hidden rather than
    /// shown in a state we cannot vouch for.
    autostart: Option<bool>,
    /// The microphone PipeWire is actually handing the daemon.
    input: Option<String>,
    entries: Vec<history::Entry>,
    /// One read-only editor per entry, purely so its transcript can be mouse-
    /// selected and copied - iced has no plain selectable text widget.
    history_editors: Vec<text_editor::Content>,
    /// Word counts for the Overview activity calendar, oldest day first.
    daily_words: Vec<u32>,
    /// Result of the last update check. Starts Unknown: opening a settings
    /// window should not put a network call in the path of flipping a switch.
    update: update::Status,
    /// True while the release tarball is downloading and installing.
    updating: bool,
    models: Vec<system::Model>,
    /// True while `flow install` is running in the background.
    installing_models: bool,
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
    hover_at: std::time::Instant,
    /// When each toggle last flipped, so its knob can travel rather than jump.
    toggled_at: std::collections::HashMap<&'static str, std::time::Instant>,
}

impl Console {
    fn new() -> (Self, Task<Message>) {
        let entries = history::recent();
        let history_editors = history_editors(&entries);
        (
            Self {
                section: Section::Overview,
                daemon: daemon::State::default(),
                settings: settings::Settings::load(),
                save_error: None,
                saved: false,
                autostart: system::autostart_enabled(),
                input: system::default_input(),
                entries,
                history_editors,
                daily_words: history::daily_words(CALENDAR_DAYS),
                update: update::Status::default(),
                updating: false,
                models: system::models(),
                installing_models: false,
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
                hover_at: std::time::Instant::now(),
                toggled_at: std::collections::HashMap::new(),
            },
            Task::none(),
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
            || self
                .toggled_at
                .values()
                .any(|at| running(*at, KNOB))
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

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Select(section) => self.section = section,
            Message::Tick(now) => self.now = now,
            Message::Hover(section) => {
                if self.hovered != section {
                    self.hovered = section;
                    self.hover_at = std::time::Instant::now();
                }
            }
            Message::PushToTalk(on) => {
                self.settings.push_to_talk = on;
                self.toggled_at.insert("push_to_talk", std::time::Instant::now());
                self.persist();
            }
            Message::Cleanup(on) => {
                self.settings.cleanup = on;
                self.toggled_at.insert("cleanup", std::time::Instant::now());
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
                    self.history_editors = history_editors(&self.entries);
                    self.daily_words = history::daily_words(CALENDAR_DAYS);
                }
            }
            Message::HistoryAction(index, action) => {
                // Read-only: every action but editing is let through, so the
                // mouse can still select and Ctrl+C still copies.
                if let Some(content) = self.history_editors.get_mut(index) {
                    if !matches!(action, text_editor::Action::Edit(_)) {
                        content.perform(action);
                    }
                }
            }
            Message::Daemon(daemon::Event::Disconnected) => {
                self.daemon = daemon::State::default()
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
                    async move {
                        tokio_free_capture(cancelled)
                    },
                    Message::Captured,
                );
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
                if let Err(err) = system::service(verb) {
                    self.save_error = Some(err);
                }
            }
            Message::OpenConfig => {
                if let Err(err) = system::open(&settings::config_path()) {
                    self.save_error = Some(err);
                }
            }
            Message::InstallModels => {
                if !self.installing_models {
                    self.installing_models = true;
                    self.save_error = None;
                    return Task::perform(
                        async { system::install_models() },
                        Message::ModelsInstalled,
                    );
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
            Message::ModelsInstalled(result) => {
                self.installing_models = false;
                match result {
                    Ok(()) => self.models = system::models(),
                    Err(err) => self.save_error = Some(err),
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
                container(text(env!("CARGO_PKG_VERSION")).size(11).font(Font::MONOSPACE).color(FAINT))
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
        container(content)
            .width(Fill)
            .height(Fill)
            .padding([34, 36])
            .into()
    }

    /// The landing page: is Flow running, and the handful of numbers that say
    /// whether it has been doing anything. Everything else in the rail is
    /// either a log (History) or a setting - this is the one page that is
    /// just a status report, laid out as a dashboard rather than a plain list
    /// so the numbers that matter are legible at a glance.
    fn overview_section(&self) -> Element<'_, Message> {
        let running = self.daemon.activity != daemon::Activity::Offline;
        let installed = self.models.iter().filter(|m| m.installed).count();
        let (label, dot) = activity_label(self.daemon.activity);

        let status_row = row![
            pip(dot),
            Space::new().width(8),
            text(label).size(13).color(FG),
            Space::new().width(Fill),
            action_msg(
                if running { "Restart" } else { "Start" },
                false,
                Message::Service(if running { "restart" } else { "start" }),
            ),
        ]
        .align_y(iced::Center);

        let status: Element<Message> = match &self.daemon.problem {
            Some(problem) => column![
                status_row,
                Space::new().height(8),
                text(problem.clone()).size(12).color(ERR),
            ]
            .into(),
            None => status_row.into(),
        };

        // However much of the calendar buffer is actually worth showing: a
        // fresh install has one or two active days, and a fixed half-year
        // grid would bury them in blank squares. Grows with real usage, up
        // to the full buffer.
        let oldest_at = self.entries.last().map_or(history::now(), |entry| entry.at);
        let history_days = (history::now().saturating_sub(oldest_at) / 86_400 + 1) as usize;
        let span = history_days.clamp(14, self.daily_words.len());
        let counts = &self.daily_words[self.daily_words.len() - span..];
        let total_words: u32 = counts.iter().sum();

        let kpis = column![
            kpi_row(
                stat_tile(self.entries.len().to_string(), "Dictations kept"),
                stat_tile(total_words.to_string(), "Words dictated"),
                stat_tile(active_days(counts).to_string(), "Active days"),
            ),
            Space::new().height(16),
            kpi_row(
                stat_tile(plural_days(current_streak(counts)), "Current streak"),
                stat_tile(plural_days(longest_streak(counts)), "Longest streak"),
                stat_tile(format!("{installed}/{}", self.models.len()), "Models installed"),
            ),
        ];

        let calendar = calendar_card(counts);

        column![
            text("Overview").size(22).color(FG),
            Space::new().height(10),
            text("How Flow is doing right now.").size(13).color(MUTED),
            Space::new().height(24),
            status,
            Space::new().height(20),
            kpis,
            Space::new().height(16),
            calendar,
        ]
        .into()
    }

    /// What was dictated, newest first.
    ///
    /// No live activity here on purpose. You dictate with the keybinding, not
    /// by looking at this window - so a "listening" indicator on a screen you
    /// are not looking at says nothing. What is worth opening the window for
    /// is what it actually wrote.
    fn history_section(&self) -> Element<'_, Message> {
        let now = history::now();

        let mut list = column![];
        if self.entries.is_empty() {
            list = list.push(
                text("Nothing yet. Hold the chord and say something.")
                    .size(13)
                    .color(FAINT),
            );
        } else {
            for (index, entry) in self.entries.iter().enumerate() {
                let when = history::ago(entry.at, now);
                list = list.push(
                    container(
                        column![
                            selectable_line(&self.history_editors[index], index),
                            Space::new().height(4),
                            text(if when.is_empty() {
                                format!("{:.1}s", entry.spoken)
                            } else {
                                format!("{:.1}s  ·  {when}", entry.spoken)
                            })
                            .size(11)
                            .font(Font::MONOSPACE)
                            .color(FAINT),
                        ]
                    )
                    .padding([10, 0]),
                );
                if index + 1 < self.entries.len() {
                    list = list.push(hairline());
                }
            }
        }

        column![
            text("History").size(22).color(FG),
            Space::new().height(10),
            text("Everything Flow has typed for you, most recent first.")
                .size(13)
                .color(MUTED),
            Space::new().height(26),
            scroll(container(list).padding(iced::Padding::default().right(16))),
        ]
        .into()
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
                "Held down while you speak. Applies straight away.",
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
                "Clean up transcript",
                "Removes filler and fixes punctuation with the local model. Turning it back on needs a restart.",
                toggle(self.settings.cleanup, self.travel("cleanup"), Message::Cleanup),
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
                value_slider(0..=100, self.settings.duck, Message::Duck, &format!("{}%", self.settings.duck)),
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
            Some(self.save_note()),
        )
    }

    /// The vocabulary, edited here rather than in a text editor. The file is
    /// the daemon's interface; it should not have to be the user's.
    fn vocabulary_section(&self) -> Element<'_, Message> {
        let mut list = column![];
        if self.terms.is_empty() {
            list = list.push(
                text("No terms yet. Add the words Flow keeps mishearing.")
                    .size(13)
                    .color(FAINT),
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
                    .padding([6, 0]),
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
                        radius: 6.0.into(),
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

        column![
            text("Vocabulary").size(22).color(FG),
            Space::new().height(10),
            text("Names and jargon the recogniser gets wrong. One per line, spelled the way you want it written.")
                .size(13)
                .color(MUTED),
            Space::new().height(22),
            entry,
            Space::new().height(10),
            note,
            Space::new().height(20),
            scroll(container(list).padding(iced::Padding::default().right(16))),
        ]
        .into()
    }

    fn models_section(&self) -> Element<'_, Message> {
        // Bound so the borrows in the rows outlive their construction.
        let sizes: Vec<String> = self
            .models
            .iter()
            .map(|model| system::human_bytes(model.bytes))
            .collect();

        let rows: Vec<Element<Message>> = self
            .models
            .iter()
            .zip(&sizes)
            .map(|(model, size)| {
                model_row(model.label, model.detail.clone(), size.clone(), model.installed)
            })
            .collect();


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
            Some({
                let mut footer = row![
                    text(total).size(12)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                    Space::new().width(Fill),
                ];
                if !all_installed {
                    footer = footer.push(action_msg(
                        if self.installing_models { "Installing…" } else { "Install models" },
                        true,
                        Message::InstallModels,
                    ));
                }
                footer.align_y(iced::Center).into()
            }),
        )
    }

    fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        let config = settings::config_path().display().to_string();
        let history_file = history::path().display().to_string();
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
            fact_row("Session", self.session.clone()),
            fact_row("Config", config),
            fact_row("History", history_file),
        ];

        section_shell(
            "Flow",
            "Push-to-talk dictation that runs entirely on your own machine.",
            rows,
            Some(
                row![
                    Space::new().width(Fill),
                    action_msg("Open config", false, Message::OpenConfig),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        )
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
                text(update::running())
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(MUTED),
                Space::new().width(12),
                action,
            ]
            .align_y(iced::Center),
        )
        .padding([14, 0])
        .into()
    }
}

fn history_editors(entries: &[history::Entry]) -> Vec<text_editor::Content> {
    entries
        .iter()
        .map(|entry| text_editor::Content::with_text(&entry.text))
        .collect()
}

/// A selectable, copyable line of text with no visible editor chrome - so it
/// reads exactly like the plain `text()` it replaces, mouse selection and
/// Ctrl+C aside.
fn selectable_line<'a>(
    content: &'a text_editor::Content,
    index: usize,
) -> Element<'a, Message> {
    text_editor(content)
        .on_action(move |action| Message::HistoryAction(index, action))
        .size(13)
        .padding(0)
        .style(|_theme, _status| text_editor::Style {
            background: Background::Color(Color::TRANSPARENT),
            border: Border::default(),
            placeholder: FG,
            value: FG,
            selection: Color { a: 0.35, ..ACCENT },
        })
        .into()
}

// ---------------------------------------------------------------------------
// Shells
// ---------------------------------------------------------------------------

/// A scrollable with a thin, browser-style bar: invisible until the pointer
/// is over it, a faint hairline while hovered. Iced's default is a wide rail
/// that sits there permanently, which reads as chrome in a window this small.
fn scroll<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(content)
        .direction(scrollable::Direction::Vertical(
            scrollable::Scrollbar::new()
                .width(4)
                .margin(2)
                .scroller_width(4),
        ))
        .style(|theme, status| {
            let base = scrollable::default(theme, status);
            let scroller_colour = match status {
                scrollable::Status::Active { .. } => Color::TRANSPARENT,
                _ => FAINT,
            };
            scrollable::Style {
                vertical_rail: scrollable::Rail {
                    background: None,
                    scroller: scrollable::Scroller {
                        background: scroller_colour.into(),
                        ..base.vertical_rail.scroller
                    },
                    ..base.vertical_rail
                },
                ..base
            }
        })
        .height(Fill)
        .into()
}

/// Every settings screen is the same shape: a heading, a sentence, a list of
/// rows separated by hairlines, and an optional footer pinned to the bottom.
fn section_shell<'a>(
    title: &'a str,
    subtitle: &'a str,
    rows: Vec<Element<'a, Message>>,
    foot: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut list = column![];
    let count = rows.len();
    for (index, entry) in rows.into_iter().enumerate() {
        list = list.push(entry);
        if index + 1 < count {
            list = list.push(hairline());
        }
    }

    let mut body = column![
        text(title).size(22).color(FG),
        Space::new().height(10),
        text(subtitle).size(13).color(MUTED),
        Space::new().height(26),
        // Right padding so the scrollbar, which iced overlays on top of the
        // content rather than beside it, cannot sit over the controls.
        scroll(container(list).padding(iced::Padding::default().right(16))),
    ];

    if let Some(foot) = foot {
        body = body.push(Space::new().height(20));
        body = body.push(hairline());
        body = body.push(Space::new().height(16));
        body = body.push(foot);
    }

    body.into()
}

// ---------------------------------------------------------------------------
// Pieces
// ---------------------------------------------------------------------------

/// A rail item behaves like a button, because it is one: the whole row lights,
/// not just its label. Selection holds a permanent muted background so the
/// current section is legible at a glance, and hover raises any row toward the
/// same treatment - so the thing under the pointer looks like the thing that
/// would happen if you clicked.
///
/// `warmth` is how far into the hover this row is, 0 to 1.
fn nav(section: Section, selected: bool, warmth: f32) -> Element<'static, Message> {
    let colour = if selected { FG } else { mix(MUTED, FG, warmth) };
    // Selected sits at full weight; hover approaches it without arriving, so
    // the two never read as the same state.
    let fill = if selected { 1.0 } else { warmth * 0.55 };

    iced::widget::mouse_area(
        button(text(section.label()).size(13).color(colour))
            .width(Fill)
            .padding([6, 9])
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(mix(
                    Color::TRANSPARENT,
                    RAISED,
                    fill,
                ))),
                text_color: colour,
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::Select(section)),
    )
    .on_enter(Message::Hover(Some(section)))
    .on_exit(Message::Hover(None))
    .into()
}

/// A label and its explanation on the left, the control on the right.
fn setting<'a>(
    label: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(label).size(13.5).color(FG),
                Space::new().height(3),
                text(description).size(12).color(FAINT),
            ]
            .width(Length::FillPortion(3)),
            Space::new().width(20),
            container(control)
                .width(Length::FillPortion(2))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(iced::Center),
    )
    .padding([14, 0])
    .into()
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
        daemon::Activity::Working => ("Cleaning up your words", ACCENT),
    }
}

/// A titled tile on a raised surface - the unit the Overview grid is built
/// from, so a handful of related facts read as one glance rather than more
/// rows in the same list.
fn card<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(
        column![
            text(title).size(12.5).color(MUTED),
            Space::new().height(12),
            content,
        ]
    )
    .padding(16)
    .width(Fill)
    .style(|_theme| container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border {
            // A shade lighter than `LINE`: against the card's own colour a
            // plain `LINE` edge all but disappears, and the card needs to
            // read as a distinct surface, not a change of shade in the page.
            color: mix(LINE, FG, 0.16),
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: iced::Shadow {
            color: Color { a: 0.35, ..Color::BLACK },
            offset: iced::Vector::new(0.0, 2.0),
            blur_radius: 10.0,
        },
        ..Default::default()
    })
    .into()
}

/// A number and its label, big and coloured to draw the eye - the KPI row a
/// dashboard opens with, rather than the same numbers sitting flat in a list.
fn stat_tile(value: String, label: &'static str) -> Element<'static, Message> {
    card(label, text(value).size(26).color(ACCENT).into())
}

/// "1 day" / "3 days" - spelled out rather than "3d", so a streak tile reads
/// as a sentence fragment and not an abbreviation to decode.
fn plural_days(count: usize) -> String {
    if count == 1 {
        "1 day".to_string()
    } else {
        format!("{count} days")
    }
}

/// Three tiles evenly spaced - one KPI row of the Overview's stat grid.
fn kpi_row(
    a: Element<'static, Message>,
    b: Element<'static, Message>,
    c: Element<'static, Message>,
) -> Element<'static, Message> {
    row![a, Space::new().width(16), b, Space::new().width(16), c].into()
}

/// How many of the calendar's days had at least one word dictated.
fn active_days(counts: &[u32]) -> usize {
    counts.iter().filter(|&&c| c > 0).count()
}

/// Consecutive active days ending today, 0 if today is empty. `counts` is
/// oldest-first, so today is the last entry.
fn current_streak(counts: &[u32]) -> usize {
    counts.iter().rev().take_while(|&&c| c > 0).count()
}

/// The longest run of consecutive active days anywhere in the calendar.
fn longest_streak(counts: &[u32]) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for &count in counts {
        if count > 0 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// A cell's colour for its word count, in five clear steps rather than a
/// continuous fade - continuous blending through a muted gold reads as
/// murky, and a calendar that is supposed to be scanned at a glance needs
/// levels a glance can actually tell apart, the way GitHub's own graph does.
fn heat_color(count: u32, max: u32) -> Color {
    if count == 0 {
        // Distinctly lighter than the card, not darker: a level-0 cell still
        // has to read as "a square in the grid", not as a hole in it.
        return mix(RAISED, FG, 0.12);
    }
    let t = count as f32 / max as f32;
    let step = if t > 0.75 {
        1.0
    } else if t > 0.5 {
        0.75
    } else if t > 0.25 {
        0.5
    } else {
        0.3
    };
    mix(RAISED, ACCENT, step)
}

/// A single legend/grid cell, shared so the legend swatches are pixel-for-
/// pixel the same shape as the grid they are explaining.
fn heat_cell(colour: Color, size: f32) -> Element<'static, Message> {
    container(Space::new())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_theme| container::Style {
            background: Some(Background::Color(colour)),
            border: Border {
                radius: (size * 0.22).into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// The Overview's activity calendar: one cell per day, coloured by how many
/// words were dictated that day, weeks as columns and weekdays as rows - the
/// GitHub contribution graph shape, because it is the shape people already
/// know how to read at a glance. `counts` is oldest-first and sized to
/// however much history is actually worth showing - a brand new install
/// should not open onto six blank months.
fn calendar_card(counts: &[u32]) -> Element<'static, Message> {
    const CELL: f32 = 13.0;

    let days = counts.len();
    let today = history::now() / 86_400;
    let first_day = today.saturating_sub(days as u64 - 1);
    // 0 = Sunday, matching `history::daily_words`'s UTC day boundaries.
    let lead = ((first_day + 4) % 7) as usize;
    let columns = (lead + days).div_ceil(7);
    let max = counts.iter().copied().max().unwrap_or(0).max(1);

    let mut grid = row![].spacing(4);
    for week in 0..columns {
        let mut col = column![].spacing(4);
        for weekday in 0..7 {
            let slot = week * 7 + weekday;
            let cell = if slot < lead || slot - lead >= days {
                Space::new()
                    .width(Length::Fixed(CELL))
                    .height(Length::Fixed(CELL))
                    .into()
            } else {
                heat_cell(heat_color(counts[slot - lead], max), CELL)
            };
            col = col.push(cell);
        }
        grid = grid.push(col);
    }

    let legend = row![
        text("Less").size(11).color(FAINT),
        Space::new().width(6),
        heat_cell(heat_color(0, 4), 10.0),
        heat_cell(heat_color(1, 4), 10.0),
        heat_cell(heat_color(2, 4), 10.0),
        heat_cell(heat_color(3, 4), 10.0),
        heat_cell(heat_color(4, 4), 10.0),
        Space::new().width(6),
        text("More").size(11).color(FAINT),
    ]
    .spacing(4)
    .align_y(iced::Center);

    let total: u32 = counts.iter().sum();
    let active = active_days(counts);
    let caption = if active == 0 {
        "Nothing dictated yet. Hold the chord and say something.".to_string()
    } else {
        format!("{total} words across {active} of the last {days} days")
    };

    card(
        "Dictation activity",
        column![
            scroll_x(grid),
            Space::new().height(12),
            row![
                text(caption).size(12).color(FAINT),
                Space::new().width(Fill),
                legend,
            ]
            .align_y(iced::Center),
        ]
        .into(),
    )
}

/// A horizontal scrollable with the same thin, hover-only bar as `scroll`,
/// for the calendar: it should fit most window widths, but must not clip
/// silently on the narrowest ones.
fn scroll_x<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scrollable(content)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(4)
                .margin(2)
                .scroller_width(4),
        ))
        .style(|theme, status| {
            let base = scrollable::default(theme, status);
            let scroller_colour = match status {
                scrollable::Status::Active { .. } => Color::TRANSPARENT,
                _ => FAINT,
            };
            scrollable::Style {
                horizontal_rail: scrollable::Rail {
                    background: None,
                    scroller: scrollable::Scroller {
                        background: scroller_colour.into(),
                        ..base.horizontal_rail.scroller
                    },
                    ..base.horizontal_rail
                },
                ..base
            }
        })
        .width(Fill)
        .into()
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
        update::Status::Installed(tag) => {
            (OK, format!("{tag} installed - restart Flow to run it"))
        }
        update::Status::Failed(why) => (ERR, format!("could not check: {why}")),
    }
}

/// A read-only pair, for About. Same rhythm as `setting` without a control.
fn fact_row(label: &'static str, value: impl Into<String>) -> Element<'static, Message> {
    container(
        row![
            text(label).size(13.5).color(FG),
            Space::new().width(Fill),
            text(value.into()).size(12).font(Font::MONOSPACE).color(MUTED),
        ]
        .align_y(iced::Center),
    )
    .padding([14, 0])
    .into()
}

fn model_row(
    label: &'static str,
    detail: impl Into<String>,
    size: impl Into<String>,
    installed: bool,
) -> Element<'static, Message> {
    container(
        row![
            column![
                text(label).size(13.5).color(FG),
                Space::new().height(3),
                text(detail.into()).size(12).font(Font::MONOSPACE).color(FAINT),
            ]
            .width(Length::FillPortion(3)),
            Space::new().width(20),
            container(
                row![
                    text(size.into()).size(12).font(Font::MONOSPACE).color(FAINT),
                    Space::new().width(14),
                    pip(if installed { FAINT } else { ACCENT }),
                    Space::new().width(7),
                    text(if installed { "Installed" } else { "Missing" })
                        .size(12)
                        .color(MUTED),
                ]
                .align_y(iced::Center),
            )
            .width(Length::FillPortion(2))
            .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(iced::Center),
    )
    .padding([14, 0])
    .into()
}

/// A toggle whose knob travels rather than teleports.
///
/// Built by hand because iced's toggler redraws instantly from its boolean and
/// has nowhere to put a position. The knob is placed by two flexible spaces
/// either side of it, so its travel is just the ratio between them, and the
/// track colour crosses over on the same curve.
///
/// `travel` is 0 at the moment of the click and 1 when it has arrived; the
/// knob moves toward `value` over that, so a toggle flipped back mid-flight
/// simply reverses.
fn toggle(value: bool, travel: f32, on_change: fn(bool) -> Message) -> Element<'static, Message> {
    let at = if value { travel } else { 1.0 - travel };
    let left = (at * 1000.0) as u16;

    let knob = container(Space::new())
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(mix(MUTED, ON_ACCENT, at))),
            border: Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let track = container(
        row![
            Space::new().width(Length::FillPortion(left.max(1))),
            knob,
            Space::new().width(Length::FillPortion((1000 - left).max(1))),
        ]
        .align_y(iced::Center),
    )
    .width(Length::Fixed(34.0))
    .height(Length::Fixed(18.0))
    .padding([3, 3])
    .style(move |_| container::Style {
        background: Some(Background::Color(mix(LINE, ACCENT, at))),
        border: Border {
            radius: 9.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    button(track)
        .padding(0)
        .style(ghost)
        .on_press(on_change(!value))
        .into()
}

/// A slider with its value beside it, so the number is always readable rather
/// than something you infer from a handle position.
fn value_slider<'a>(
    range: std::ops::RangeInclusive<u32>,
    value: u32,
    on_change: fn(u32) -> Message,
    label: &str,
) -> Element<'a, Message> {
    row![
        container(
            slider(range, value, on_change)
                .height(14)
                .style(|_theme, _status| slider::Style {
                    rail: slider::Rail {
                        backgrounds: (Background::Color(ACCENT), Background::Color(LINE)),
                        width: 2.0,
                        border: Border::default(),
                    },
                    handle: slider::Handle {
                        shape: slider::HandleShape::Circle { radius: 5.0 },
                        background: Background::Color(FG),
                        border_width: 0.0,
                        border_color: Color::TRANSPARENT,
                    },
                })
        )
        .width(Length::Fixed(140.0)),
        Space::new().width(12),
        container(
            text(label.to_string())
                .size(12)
                .font(Font::MONOSPACE)
                .color(MUTED)
        )
        .width(Length::Fixed(56.0))
        .align_x(iced::alignment::Horizontal::Right),
    ]
    .align_y(iced::Center)
    .into()
}

/// A 7px dot. The only place the accent appears besides a primary button.
fn pip(colour: Color) -> Element<'static, Message> {
    container(Space::new().width(0))
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(colour)),
            border: Border {
                radius: 3.5.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

fn vertical_hairline() -> Element<'static, Message> {
    container(Space::new().height(Fill))
        .width(Length::Fixed(1.0))
        .height(Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(LINE)),
            ..Default::default()
        })
        .into()
}

fn hairline() -> Element<'static, Message> {
    container(Space::new().width(Fill))
        .height(Length::Fixed(1.0))
        .style(|_| container::Style {
            background: Some(Background::Color(LINE)),
            ..Default::default()
        })
        .into()
}

fn action_msg(label: &str, primary: bool, on_press: Message) -> Element<'static, Message> {
    button(
        text(label.to_string())
            .size(13)
            .color(if primary { ON_ACCENT } else { FG }),
    )
    .padding([7, 14])
    .style(move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: primary.then_some(Background::Color(ACCENT)),
            text_color: if primary { ON_ACCENT } else { FG },
            border: Border {
                color: if primary {
                    ACCENT
                } else if hovered {
                    FAINT
                } else {
                    LINE
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        }
    })
    .on_press(on_press)
    .into()
}

/// Text that behaves like a link: no chrome at all, just the label.
fn ghost(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: FG,
        border: Border::default(),
        ..Default::default()
    }
}

/// Read the keyboard until a chord arrives, on whatever thread the runtime
/// gives us. Split out so the async block above stays a one-liner.
fn tokio_free_capture(cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Option<String> {
    chord::capture(&|| cancelled.load(std::sync::atomic::Ordering::Relaxed))
}
