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

mod daemon;
mod history;
mod settings;
mod system;
mod update;
mod vocabulary;

use iced::widget::{
    button, column, container, responsive, row, scrollable, slider, text, text_editor, tooltip,
    Space,
};
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
/// The lifted surface a card sits on. One step off the ground, no more.
const RAISED: Color = Color { r: 0.102, g: 0.110, b: 0.125, a: 1.0 }; // #1A1C20
/// What a rail item sits on when it is the current section. `RAISED` was doing
/// this job too, and at card weight against the same ground the selected item
/// was nearly invisible - a rail should stay quiet, but quiet still has to be
/// legible.
const RAIL_ON: Color = Color { r: 0.145, g: 0.157, b: 0.176, a: 1.0 }; // #25282D
/// Any line drawn *on* a card - its border, and any rule inside it.
///
/// `LINE` is a page-ground colour and is one value off `RAISED`: a hairline in
/// `LINE` on a card is invisible, which is worse than having no rule at all,
/// because the space it was given to occupy stays behind and reads as two
/// halves of a card drifting apart. The border already knew this; the rule did
/// not, so both now come from here and cannot drift.
const EDGE: Color = Color { r: 0.238, g: 0.248, b: 0.263, a: 1.0 }; // #3D3F43
const ACCENT: Color = Color { r: 0.180, g: 0.835, b: 0.451, a: 1.0 }; // #2ED573
const ERR: Color = Color { r: 0.831, g: 0.451, b: 0.420, a: 1.0 }; // #D4736B
/// "Nothing to do" - and the same green as ACCENT, deliberately. It used to be
/// a muted olive of its own, to sit at ERR's weight in a row of dots, and next
/// to the accent it just read as a second, dirtier green. One green in the
/// product, one meaning per colour: green is fine, red is not.
const OK: Color = ACCENT;
const ON_ACCENT: Color = Color { r: 0.078, g: 0.082, b: 0.059, a: 1.0 };

const RAIL_WIDTH: f32 = 176.0;

/// How far back the Overview keeps daily counts. Not the number of weeks on
/// screen: the calendar draws as many weeks as the pane is wide, so this is
/// the ceiling for a very wide window - two years, past which a dictation
/// habit is better summarised than drawn.
const CALENDAR_WEEKS: usize = 104;
const CALENDAR_DAYS: usize = CALENDAR_WEEKS * 7;

/// One calendar cell, and the gap between two of them. Fixed rather than
/// stretched to the pane: a heat cell is read by colour, and colour on a
/// square is easier to compare than colour on a rectangle that changes shape
/// with the window. The pane's width buys more weeks instead.
const CELL: f32 = 12.0;
const CELL_GAP: f32 = 3.0;
/// The Mon/Wed/Fri gutter down the left of the grid.
const WEEKDAY_GUTTER: f32 = 28.0;

/// How much of the transcript log the Overview repeats. Enough to recognise
/// the last thing you said and no more - reading the log is History's job, and
/// every extra line here is a line that pushes the page into scrolling.
const RECENT_SHOWN: usize = 2;
/// The one gap between everything on the Overview - between cards, and
/// between the tiles in a row. A page of cards with three different gaps in it
/// reads as a page of cards that were placed one at a time.
const GAP: f32 = 12.0;

/// Where a recent line is cut. Sized to the default window rather than
/// measured: `Wrapping::None` already stops a long line becoming two, and this
/// is what puts the ellipsis somewhere deliberate.
const RECENT_CHARS: usize = 96;

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
        Section::ALL
            .into_iter()
            .find(|section| section.label().eq_ignore_ascii_case(name.trim()))
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
    /// Per-day rollup for the Overview's calendar and week numbers, oldest
    /// day first.
    days: Vec<history::Day>,
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
                section: Section::initial(),
                daemon: daemon::State::default(),
                settings: settings::Settings::load(),
                save_error: None,
                saved: false,
                autostart: system::autostart_enabled(),
                input: system::default_input(),
                entries,
                history_editors,
                days: history::daily(CALENDAR_DAYS),
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
                    self.days = history::daily(CALENDAR_DAYS);
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

        // The Overview is one long scroll and pads itself, because a scroll
        // viewport clips at the frame: with the breathing room outside the
        // scrollable, the first and last card get sliced off at a hard edge
        // 34px in, which reads as a broken layout rather than as more content.
        // Inside, the padding scrolls with the cards and the ends look like
        // ends. Every other section pins its own header and scrolls only the
        // rows under it, so the frame is the right place for their padding.
        let vertical = if self.section == Section::Overview { 0 } else { 34 };

        // Switching sections is deliberately instant. Motion here read as the
        // page arriving late rather than as polish - navigation should feel
        // like the content was already there.
        container(content)
            .width(Fill)
            .height(Fill)
            .padding([vertical, 36])
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

        let header = row![
            text("Overview").size(22).color(FG),
            Space::new().width(Fill),
            pip(dot),
            Space::new().width(9),
            text(label).size(13).color(MUTED),
            Space::new().width(16),
            action_msg(
                if running { "Restart" } else { "Start" },
                !running,
                Message::Service(if running { "restart" } else { "start" }),
            ),
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
                (format!("{active} of 7 days active"), FAINT),
            ),
            Space::new().width(GAP),
            stat_tile(
                "Speaking time",
                history::duration(spoken),
                if dictations == 0 {
                    ("nothing this week".to_string(), FAINT)
                } else {
                    (
                        format!("{} average", history::duration(spoken / dictations as f32)),
                        FAINT,
                    )
                },
            ),
            Space::new().width(GAP),
            stat_tile(
                "Current streak",
                plural(streak as u32, "day"),
                (
                    format!("longest {}", plural(longest_streak(&self.days) as u32, "day")),
                    FAINT,
                ),
            ),
        ];

        let body = column![
            header,
            Space::new().height(GAP),
            self.setup_card(installed),
            Space::new().height(GAP),
            kpis,
            Space::new().height(GAP),
            calendar_card(&self.days),
            Space::new().height(GAP),
            self.recent_card(),
        ];

        // Sized to fit the default window, so the common case does not scroll
        // at all. Scrolled rather than compressed when it does not fit: a user
        // who drags the window down to the minimum height should have to reach
        // the last card rather than have every card shrink to meet them.
        scroll(container(body).padding(
            iced::Padding::default().top(28.0).bottom(28.0).right(14.0),
        ))
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
            fact(
                "Microphone",
                clip(
                    self.input.as_deref().unwrap_or("system default"),
                    38,
                ),
            ),
            Space::new().width(Fill),
            fact("Models", format!("{installed} of {}", self.models.len())),
        ];

        let mut body = column![];
        let notes = self.attention(installed);
        for (colour, note) in &notes {
            body = body.push(text(note.clone()).size(12).color(*colour));
            body = body.push(Space::new().height(8));
        }
        if !notes.is_empty() {
            body = body.push(card_rule());
            body = body.push(Space::new().height(13));
        }

        panel(body.push(facts).into())
    }

    /// Everything on this page that is worth doing something about, in the
    /// order it would bite. Empty when there is nothing to do, and the status
    /// card then collapses to one line - a dashboard that always has a row of
    /// warnings in it teaches people not to read the warnings.
    fn attention(&self, installed: usize) -> Vec<(Color, String)> {
        let mut notes = Vec::new();
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

    /// The last few transcripts, because the fastest way to tell whether Flow
    /// is doing a good job is to read what it wrote. Not selectable here on
    /// purpose - this is a glance, and History is one click away for the copy.
    fn recent_card(&self) -> Element<'_, Message> {
        let now = history::now();

        let mut list = column![];
        if self.entries.is_empty() {
            list = list.push(
                text("Nothing yet. Hold the chord and say something.")
                    .size(13)
                    .color(FAINT),
            );
        } else {
            for (index, entry) in self.entries.iter().take(RECENT_SHOWN).enumerate() {
                if index > 0 {
                    list = list.push(hairline());
                }
                list = list.push(
                    container(
                        row![
                            text(one_line(&entry.text))
                                .size(13)
                                .color(FG)
                                .wrapping(text::Wrapping::None)
                                .width(Fill),
                            Space::new().width(12),
                            text(history::ago(entry.at, now)).size(11).color(FAINT),
                        ]
                        .align_y(iced::Center),
                    )
                    .padding([9, 0]),
                );
            }
        }

        panel(
            column![
                row![
                    text("Recent dictations").size(12.5).color(MUTED),
                    Space::new().width(Fill),
                    button(text("All history").size(12).color(MUTED))
                        .padding(0)
                        .style(ghost)
                        .on_press(Message::Select(Section::History)),
                ]
                .align_y(iced::Center),
                Space::new().height(6),
                list,
            ]
            .into(),
        )
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
                // The path takes what is left over, rather than a Fill space
                // taking it: the path is the flexible half of this row and the
                // button is the fixed half, and a wrapping text asked to
                // shrink will happily claim the whole row first.
                let mut footer = row![
                    text(total)
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(FAINT)
                        .width(Fill),
                    Space::new().width(12),
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
    // the two never read as the same state. 0.7 rather than 0.55 because a
    // hover you have to look for is a hover that is not doing its job.
    let fill = if selected { 1.0 } else { warmth * 0.7 };

    iced::widget::mouse_area(
        button(text(section.label()).size(13).color(colour))
            .width(Fill)
            .padding([6, 9])
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(mix(
                    Color::TRANSPARENT,
                    RAIL_ON,
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

/// A raised surface with a hairline edge - the unit every Overview card is
/// built from, so a handful of related facts read as one glance rather than
/// more rows in the same list.
fn panel<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .padding(14)
        .width(Fill)
        .style(|_theme| container::Style {
            background: Some(Background::Color(RAISED)),
            border: Border {
                color: EDGE,
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

/// A panel with its subject named along the top.
fn card<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    panel(
        column![
            text(title).size(12.5).color(MUTED),
            Space::new().height(12),
            content,
        ]
        .into(),
    )
}

/// A number with what it is above it and what it means below it.
///
/// The second line is the whole point. "42" says nothing; "42, five of seven
/// days active" says whether this was a busy week. The value is plain `FG`
/// rather than the accent - four accent numbers in a row is a row of alarms,
/// and the accent's one job in this product is "live".
fn stat_tile(
    label: &'static str,
    value: String,
    note: (String, Color),
) -> Element<'static, Message> {
    let (note, colour) = note;
    panel(
        column![
            text(label).size(12).color(MUTED),
            Space::new().height(9),
            text(value).size(25).color(FG),
            Space::new().height(5),
            text(note).size(11).color(colour),
        ]
        .into(),
    )
}

/// This week against last week, as the second line of a KPI tile.
///
/// A percentage needs something to be a percentage of, so a first week says so
/// instead of dividing by zero. A rise takes the accent and a fall does not:
/// dictating less in a week is not a fault to flag in red.
fn trend(now: u32, before: u32) -> (String, Color) {
    if before == 0 {
        return if now == 0 {
            ("nothing yet".to_string(), FAINT)
        } else {
            ("first week with words".to_string(), ACCENT)
        };
    }
    let change = (now as i64 - before as i64) * 100 / before as i64;
    match change {
        0 => ("level with last week".to_string(), FAINT),
        up if up > 0 => (format!("+{up}% vs last week"), ACCENT),
        down => (format!("{down}% vs last week"), FAINT),
    }
}

/// A small label with its value under it. Sized to its content: the row packs
/// these left with deliberate gaps rather than stretching each to an equal
/// share of the width.
fn fact(label: &'static str, value: String) -> Element<'static, Message> {
    column![
        text(label).size(11).color(FAINT),
        Space::new().height(4),
        text(value)
            .size(12.5)
            .font(Font::MONOSPACE)
            .color(MUTED)
            .wrapping(text::Wrapping::None),
    ]
    .into()
}

/// `18402` -> `"18,402"`. Four figures of words dictated is a real number to
/// reach, and it should not have to be counted digit by digit.
fn commas(count: u32) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Cut to `chars` with an ellipsis, so a long value says it continues rather
/// than looking like it simply stopped. Used with `Wrapping::None`, which
/// stops the line becoming two but says nothing about where it ends.
fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(chars).collect();
    format!("{}…", cut.trim_end())
}

/// One line of a transcript, for the Overview's recent list.
fn one_line(text: &str) -> String {
    clip(text.trim().replace('\n', " ").as_str(), RECENT_CHARS)
}

/// How many of these days had at least one word dictated.
fn active_days(days: &[history::Day]) -> usize {
    days.iter().filter(|day| day.words > 0).count()
}

/// Consecutive active days up to now. `days` is oldest-first, so today is the
/// last entry.
///
/// An empty *today* does not end the streak: the console gets opened in the
/// morning before anything has been dictated, and reporting a ten-day habit as
/// "0 days" because it is 9am would be wrong in the only way that matters. Two
/// empty days in a row does end it.
fn current_streak(days: &[history::Day]) -> usize {
    let ending_today = days.iter().rev().take_while(|day| day.words > 0).count();
    if ending_today > 0 || days.len() < 2 {
        return ending_today;
    }
    days[..days.len() - 1]
        .iter()
        .rev()
        .take_while(|day| day.words > 0)
        .count()
}

/// The longest run of consecutive active days anywhere in the buffer.
fn longest_streak(days: &[history::Day]) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for day in days {
        if day.words > 0 {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// The word count a cell has to reach to be drawn at full heat.
///
/// Not the busiest day: one afternoon spent dictating a document is several
/// times a normal day, and scaling to it flattens every ordinary day to the
/// palest step - a graph where nothing is distinguishable except the outlier.
/// The 80th percentile of *active* days puts the top of the scale inside
/// normal use and lets the exceptional days simply saturate.
fn heat_ceiling(days: &[history::Day]) -> u32 {
    let mut active: Vec<u32> = days.iter().map(|day| day.words).filter(|&w| w > 0).collect();
    if active.is_empty() {
        return 1;
    }
    active.sort_unstable();
    active[(active.len() * 4 / 5).min(active.len() - 1)].max(1)
}

/// A cell's colour for its word count, in four clear steps rather than a
/// continuous fade - continuous blending reads as murky, and a calendar that
/// is supposed to be scanned at a glance needs levels a glance can actually
/// tell apart, the way GitHub's own graph does.
fn heat_color(count: u32, ceiling: u32) -> Color {
    if count == 0 {
        // Distinctly lighter than the card, not darker: a level-0 cell still
        // has to read as "a square in the grid", not as a hole in it.
        return mix(RAISED, FG, 0.12);
    }
    let t = (count as f32 / ceiling as f32).min(1.0);
    let step = if t > 0.75 {
        1.0
    } else if t > 0.5 {
        0.72
    } else if t > 0.25 {
        0.48
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

/// One day of the grid, with what it was on hover. The tooltip is what turns
/// the graph from decoration into something you can read a date off.
fn day_cell(day: history::Day, number: u64, ceiling: u32) -> Element<'static, Message> {
    let when = history::short_date(number);
    let what = match day.words {
        0 => "nothing dictated".to_string(),
        1 => "1 word".to_string(),
        words => format!("{} words in {}", commas(words), plural(day.dictations, "dictation")),
    };

    tooltip(
        heat_cell(heat_color(day.words, ceiling), CELL),
        container(text(format!("{when} · {what}")).size(11.5).color(FG))
            .padding([5, 8])
            .style(|_theme| container::Style {
                background: Some(Background::Color(BG)),
                border: Border {
                    color: mix(LINE, FG, 0.22),
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            }),
        tooltip::Position::Top,
    )
    .gap(6)
    .into()
}

/// The Overview's activity calendar: one cell per day, coloured by how many
/// words were dictated, weeks as columns and weekdays as rows - the GitHub
/// contribution graph shape, because it is the shape people already know how
/// to read at a glance.
///
/// How many weeks are drawn comes from how wide the pane is, not from how much
/// history exists. A grid sized to the history is a grid that ends in the
/// middle of its own card, which is what made this look broken rather than
/// empty; a full grid with a quiet left half reads correctly as "before you
/// started". `days` is oldest-first and holds the whole buffer.
fn calendar_card(days: &[history::Day]) -> Element<'_, Message> {
    // Month labels, then the grid. Both fixed, so the card does not need the
    // responsive width to know how tall it is.
    const MONTH_ROW: f32 = 15.0;
    let grid_height = 7.0 * CELL + 6.0 * CELL_GAP;

    let ceiling = heat_ceiling(days);
    let today = history::now() / 86_400;

    let grid = responsive(move |size| {
        // Whole columns that fit beside the weekday gutter, capped by the
        // buffer. The floor keeps a very narrow window from drawing a stub.
        let pitch = CELL + CELL_GAP;
        let columns = (((size.width - WEEKDAY_GUTTER + CELL_GAP) / pitch).floor() as usize)
            .clamp(8, CALENDAR_WEEKS);

        // The grid ends on today, so the last column is a partial week and
        // every column before it is a full Sunday-to-Saturday one.
        // 0 = Sunday, matching `history::daily`'s UTC day boundaries.
        let last_weekday = ((today + 4) % 7) as usize;
        let first_day = today + last_weekday as u64 + 1 - (columns as u64 * 7);

        let mut weeks = row![].spacing(CELL_GAP);
        for column in 0..columns {
            let mut week = column![].spacing(CELL_GAP);
            for weekday in 0..7 {
                let number = first_day + (column * 7 + weekday) as u64;
                // The buffer's oldest day, and tomorrow onward: both are
                // frame rather than data, and drawn as a gap so the grid
                // keeps its shape without inventing squares.
                let index = (number + days.len() as u64).checked_sub(today + 1);
                let cell = match index {
                    Some(index) if number <= today && (index as usize) < days.len() => {
                        day_cell(days[index as usize], number, ceiling)
                    }
                    _ => Space::new()
                        .width(Length::Fixed(CELL))
                        .height(Length::Fixed(CELL))
                        .into(),
                };
                week = week.push(cell);
            }
            weeks = weeks.push(week);
        }

        // One label per month, placed by giving it the exact width of its own
        // run of columns - which is what keeps a label over the month it names
        // however many weeks are on screen. A month with too little of itself
        // showing keeps its space and loses its name rather than spilling over
        // the next one.
        let mut labels: Vec<Element<'static, Message>> = Vec::new();
        let mut run = 0usize;
        let mut current = history::civil(first_day).1;
        let flush = |labels: &mut Vec<Element<'static, Message>>, run: usize, month: u32| {
            if run == 0 {
                return;
            }
            let name = if run >= 3 { month_name(month) } else { "" };
            labels.push(
                container(text(name).size(11).color(FAINT))
                    .width(Length::Fixed(run as f32 * pitch - CELL_GAP))
                    .into(),
            );
        };
        for column in 0..columns {
            let month = history::civil(first_day + (column * 7) as u64).1;
            if month != current {
                flush(&mut labels, run, current);
                run = 0;
                current = month;
            }
            run += 1;
        }
        flush(&mut labels, run, current);
        let months = iced::widget::Row::with_children(labels).spacing(CELL_GAP);

        column![
            row![
                Space::new().width(Length::Fixed(WEEKDAY_GUTTER)),
                months,
            ],
            Space::new().height(MONTH_ROW - 11.0),
            row![weekday_gutter(), weeks],
        ]
        .into()
    })
    .height(Length::Fixed(grid_height + MONTH_ROW + 4.0));

    let words = history::words(days);
    let active = active_days(days);
    let caption = if active == 0 {
        "Nothing dictated yet. Hold the chord and say something.".to_string()
    } else {
        format!(
            "{} words over {}, most recently {}",
            commas(words),
            plural(active as u32, "active day"),
            history::short_date(latest_day(days, history::now() / 86_400)),
        )
    };

    card(
        "Dictation activity",
        column![
            grid,
            Space::new().height(14),
            row![
                text(caption).size(12).color(FAINT),
                Space::new().width(Fill),
                legend(),
            ]
            .align_y(iced::Center),
        ]
        .into(),
    )
}

/// Mon/Wed/Fri only. Seven labels down a 13-pixel row pitch is a wall of text
/// beside the thing it is labelling; three is enough to orient a reader who
/// wants to know which row is which.
fn weekday_gutter() -> Element<'static, Message> {
    let mut gutter = column![].spacing(CELL_GAP);
    for weekday in 0..7 {
        let name = match weekday {
            1 => "Mon",
            3 => "Wed",
            5 => "Fri",
            _ => "",
        };
        gutter = gutter.push(
            container(text(name).size(10).color(FAINT))
                .width(Length::Fixed(WEEKDAY_GUTTER - CELL_GAP))
                .height(Length::Fixed(CELL))
                .align_y(iced::Center),
        );
    }
    row![gutter, Space::new().width(CELL_GAP)].into()
}

/// Less-to-more swatches, in the same shape and steps as the grid.
fn legend() -> Element<'static, Message> {
    row![
        text("Less").size(11).color(FAINT),
        Space::new().width(5),
        heat_cell(heat_color(0, 4), 10.0),
        heat_cell(heat_color(1, 4), 10.0),
        heat_cell(heat_color(2, 4), 10.0),
        heat_cell(heat_color(3, 4), 10.0),
        heat_cell(heat_color(4, 4), 10.0),
        Space::new().width(5),
        text("More").size(11).color(FAINT),
    ]
    .spacing(3)
    .align_y(iced::Center)
    .into()
}

/// The day number of the most recent active day, or today if there is none.
fn latest_day(days: &[history::Day], today: u64) -> u64 {
    days.iter()
        .rposition(|day| day.words > 0)
        .map(|index| today + index as u64 + 1 - days.len() as u64)
        .unwrap_or(today)
}

fn month_name(month: u32) -> &'static str {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    MONTHS[((month - 1) % 12) as usize]
}

/// "1 dictation" / "4 dictations". Spelled out rather than abbreviated, so a
/// caption reads as a sentence and not as something to decode.
fn plural(count: u32, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
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
                    pip(if installed { OK } else { ERR }),
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
    rule(LINE)
}

/// The same rule, in the only colour that is visible on a card.
fn card_rule() -> Element<'static, Message> {
    rule(EDGE)
}

fn rule(colour: Color) -> Element<'static, Message> {
    container(Space::new().width(Fill))
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(colour)),
            ..Default::default()
        })
        .into()
}

fn action_msg(label: &str, primary: bool, on_press: Message) -> Element<'static, Message> {
    button(
        text(label.to_string())
            .size(13)
            .color(if primary { ON_ACCENT } else { FG })
            // A button is as wide as its label, full stop. Left to wrap, an
            // "Install models" beside a long path folded onto two lines and
            // then clipped, because the row had already given the path every
            // pixel it asked for.
            .wrapping(text::Wrapping::None),
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

#[cfg(test)]
mod tests {
    use super::{commas, current_streak, heat_ceiling, one_line, trend, Section, ACCENT, FAINT};

    fn days(words: &[u32]) -> Vec<crate::history::Day> {
        words
            .iter()
            .map(|&words| crate::history::Day {
                words,
                dictations: u32::from(words > 0),
                spoken: 0.0,
            })
            .collect()
    }

    /// The streak tile is read first thing in the morning, before anything has
    /// been dictated, and it must not call a live habit dead.
    #[test]
    fn an_empty_today_does_not_break_the_streak() {
        assert_eq!(current_streak(&days(&[0, 4, 4, 4])), 3);
        assert_eq!(current_streak(&days(&[4, 4, 4, 0])), 3);
        assert_eq!(current_streak(&days(&[4, 4, 0, 0])), 0);
        assert_eq!(current_streak(&days(&[0, 0, 0, 0])), 0);
    }

    /// One long dictating afternoon must not flatten every ordinary day to the
    /// palest step, which is what scaling to the busiest day does.
    #[test]
    fn the_heat_scale_ignores_the_outlier_day() {
        let ordinary = [40, 50, 60, 55, 45, 4_000];
        assert!(heat_ceiling(&days(&ordinary)) <= 60);
        // Empty days are not part of the distribution, and an empty history
        // still needs a divisor.
        assert_eq!(heat_ceiling(&days(&[0, 0, 0])), 1);
        assert_eq!(heat_ceiling(&days(&[0, 7, 0])), 7);
    }

    #[test]
    fn a_week_is_reported_against_the_one_before_it() {
        assert_eq!(trend(120, 100).0, "+20% vs last week");
        assert_eq!(trend(80, 100).0, "-20% vs last week");
        assert_eq!(trend(100, 100), ("level with last week".to_string(), FAINT));
        assert_eq!(trend(50, 0).1, ACCENT);
        assert_eq!(trend(0, 0), ("nothing yet".to_string(), FAINT));
    }

    #[test]
    fn long_numbers_and_long_lines_stay_readable() {
        assert_eq!(commas(7), "7");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_000), "1,000");
        assert_eq!(commas(18_402), "18,402");
        assert_eq!(one_line("  two\nlines  "), "two lines");
        assert!(one_line(&"x".repeat(200)).ends_with('…'));
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
}
