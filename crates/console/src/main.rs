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
mod demo;
mod history;
mod settings;
mod system;
mod update;
mod vocabulary;

use iced::widget::{
    button, canvas, column, container, responsive, rich_text, row, scrollable, slider, span, text,
    tooltip, Canvas, Space,
};
use iced::{
    Background, Border, Color, Element, Fill, Font, Length, Point, Size, Subscription, Task, Theme,
};

// ---------------------------------------------------------------------------
// Tokens. Taken from the island in overlay.rs so the window and the overlay
// read as one product: its ground colour, its restraint, one warm accent that
// only ever means "live".
// ---------------------------------------------------------------------------

const BG: Color = Color { r: 0.039, g: 0.043, b: 0.055, a: 1.0 }; // #0A0B0E
const FG: Color = Color { r: 0.925, g: 0.929, b: 0.937, a: 1.0 }; // #ECEDEF
/// Secondary text: labels, captions, the second line of a tile. Lifted from
/// #7C828C, which sat at 4.4:1 on a card and so failed the same contrast bar
/// the body text clears comfortably. Quiet is a job for weight and size here,
/// not for a grey that has to be squinted at.
const MUTED: Color = Color { r: 0.541, g: 0.565, b: 0.604, a: 1.0 }; // #8A909A
/// The quietest text in the product - 11px meta: timestamps, month names, the
/// label half of a label/value pair. Also lifted, from #4E545C at 2.2:1, which
/// is decoration rather than text at that size.
const FAINT: Color = Color { r: 0.424, g: 0.451, b: 0.490, a: 1.0 }; // #6C737D
const LINE: Color = Color { r: 0.106, g: 0.118, b: 0.137, a: 1.0 }; // #1B1E23
/// The lifted surface a card sits on. Half a step off the ground rather than a
/// full one: a card already carries a hairline and a shadow, and three depth
/// cues on one rectangle - repeated down a page of them - is what turned the
/// Overview into a stack of grey plates. The container recedes; the words on
/// it are the thing to see.
const RAISED: Color = Color { r: 0.082, g: 0.090, b: 0.106, a: 1.0 }; // #15171B
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
///
/// A hairline, not an outline. At #3D3F43 every card was drawn as a box first
/// and read as content second - seven outlined rectangles on one page. This is
/// the lowest value that still separates a card from the ground and still
/// shows up as a rule *on* the card.
const EDGE: Color = Color { r: 0.149, g: 0.165, b: 0.184, a: 1.0 }; // #262A2F
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

/// The one gap between everything on the Overview - between cards, and
/// between the tiles in a row. A page of cards with three different gaps in it
/// reads as a page of cards that were placed one at a time.
const GAP: f32 = 12.0;

/// How much room a scrolling list keeps at each end, inside the viewport so
/// that it scrolls with the content rather than being clipped away with it.
const SCROLL_PAD: f32 = 18.0;
/// Where the first thing on a page sits. Padding inside the scroll, so it
/// is room above the heading and below the last row - it moves with them,
/// and at either end of the page there is still air.
const PAGE_TOP: f32 = 32.0;

/// The pane's left margin, and the base for the content's right margin. One
/// constant so the page reads as evenly framed even though the two sides get
/// there differently - the left is a pane pad, the right is the same room
/// plus the scrollbar's own footprint.
const PANE_INSET: f32 = 32.0;
/// Where the text stops on the right: the left margin plus clearance for the
/// scrollbar (width 4, margin 2 on each side of its track), so a visible
/// scroller never sits on top of a letter.
const CONTENT_RIGHT: f32 = PANE_INSET;
/// History keeps its text on the page grid while the hover surface reaches
/// past it. The same value is the row's inner inset and the surface's bleed.
const ENTRY_INSET: f32 = 12.0;
/// Top and bottom padding for one row in a settings list, on both sides of
/// every hairline between them. Anything else that borders a hairline - the
/// footer's, for one - uses this too, so a divider always has the same air
/// whichever two things it happens to be sitting between.
const ROW_PAD: f32 = 16.0;
/// The docked footer's band, above and below its content. Equal on both
/// sides, because a bar reads as a bar only when its content sits in the
/// middle of it - the old pairing borrowed `ROW_PAD` above and the page's
/// whole bottom margin below, which left the button hung near the hairline
/// with a stretch of dead floor under it.
const FOOT_PAD: f32 = 18.0;

/// How long each motion takes. Only two things move - a toggle's knob and a
/// rail item warming under the pointer - because those are the two that
/// acknowledge something the user just did. Long enough that the easing
/// curve is visible rather than read as a snap.
const KNOB: u64 = 220;
const FADE: u64 = 200;
/// How long a copied row keeps its paper lit before the icon goes back to
/// waiting for a hover. Long enough to be read, short enough that it is never
/// still saying it by the time you look again.
const COPIED: u64 = 1600;

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
    Cleanup(bool),
    Terminal(bool),
    Denoise(bool),
    Autostart(bool),
    Duck(u32),
    OpenConfig,
    /// Reveal a path in the file manager. About's config and history rows.
    OpenPath(std::path::PathBuf),
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
    /// Which transcript the pointer is over, so History can light the row and
    /// offer copy without those being permanent chrome.
    HoverEntry(Option<usize>),
    /// Put one transcript on the clipboard.
    Copy(usize),
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
    /// True while `flow install` is running in the background.
    installing_models: bool,
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
        (
            Self {
                section: Section::initial(),
                daemon: pretend.map_or_else(daemon::State::default, demo::daemon_state),
                settings: settings::Settings::load(),
                save_error: None,
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
                installing_models: false,
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
            Task::none(),
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

    fn update(&mut self, message: Message) -> Task<Message> {
        // The socket is the one thing the demo cannot talk over: it is either
        // absent or belongs to a real daemon, and either way its first event
        // would replace the state being looked at.
        if self.demo && matches!(message, Message::Daemon(_)) {
            return Task::none();
        }

        match message {
            Message::Select(section) => self.section = section,
            Message::Tick(now) => self.now = now,
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
                if let Err(err) = system::service(verb) {
                    self.save_error = Some(err);
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
            (None, true) => text("Saved. Applies to your next dictation.").size(12).color(FAINT),
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
                        if self.installing_models { "Installing…" } else { "Install models" },
                        true,
                        Message::InstallModels,
                    )
                }),
            )),
        )
    }

    fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
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

/// One transcript and what it cost: the line itself, then how long it took to
/// say and how long ago that was. Shared so the Overview's excerpt and the
/// History log are the same row rather than two rows that look alike.
///
/// Copy is an icon on the row, not a label and not a hidden editor. iced draws
/// a caret in the text colour on any focused `text_editor`, and there is no
/// way to style it off - so a click that was meant to select a sentence parked
/// a blinking cursor in the middle of a settings window.
///
/// The icon's slot is always in the layout, at a fixed size, and only its
/// opacity moves with the hover - it never appears or disappears as an
/// element. Swapping a `Space` in for it while hidden (the first version of
/// this) changed the row's height between the two states, which under a
/// stationary pointer flips which row it is over and made the highlight
/// flicker between rows rather than hold still on one.
///
/// The transparent gap lives inside the hover target so moving between rows
/// cannot drop the highlight.
///
/// Rows only *set* the hovered index. Clearing it is `entry_list`'s job: a
/// nested copy control used to capture the event iced's `mouse_area` needs
/// to see, so `on_exit` fired while the pointer was still on the row, and
/// crossing from one row onto the next went through `None` and dropped the
/// highlight.
fn entry_row<'a>(
    entry: &'a history::Entry,
    index: usize,
    now: u64,
    copied: bool,
    warmth: f32,
    separated: bool,
) -> Element<'a, Message> {
    let when = history::ago(entry.at, now);
    let body = container(
        row![
            column![
                text(&entry.text).size(13).color(FG),
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
            .width(Fill),
            copy_btn(index, copied, warmth),
        ]
        .align_y(iced::Center),
    )
    .padding([10.0, ENTRY_INSET])
    .width(Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(mix(Color::TRANSPARENT, RAIL_ON, warmth * 0.7))),
        border: Border { radius: 6.0.into(), ..Default::default() },
        ..Default::default()
    });

    let mut stack = column![body].width(Fill);
    if separated {
        stack = stack.push(Space::new().height(2));
    }

    iced::widget::mouse_area(stack).on_enter(Message::HoverEntry(Some(index))).into()
}

/// The list, not the row, owns "pointer left". See `entry_row`.
fn entry_list<'a>(rows: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    iced::widget::mouse_area(rows).on_exit(Message::HoverEntry(None)).into()
}

// ---------------------------------------------------------------------------
// Shells
// ---------------------------------------------------------------------------

/// A scrollable with a thin, browser-style bar: invisible until the pointer
/// is over it, a faint hairline while hovered. Iced's default is a wide rail
/// that sits there permanently, which reads as chrome in a window this small.
///
/// Top and bottom padding is on the content, not the pane: it is the air
/// above the heading and below the last row, and it scrolls with them. The
/// right pad is for the bar itself, which iced overlays on top of the
/// content rather than beside it.
fn scroll<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scroll_pad(content, PAGE_TOP)
}

fn scroll_pad<'a>(content: impl Into<Element<'a, Message>>, bottom: f32) -> Element<'a, Message> {
    scroll_inset(content, bottom, CONTENT_RIGHT)
}

fn scroll_inset<'a>(
    content: impl Into<Element<'a, Message>>,
    bottom: f32,
    right: f32,
) -> Element<'a, Message> {
    scrollable(
        container(content)
            .padding(iced::Padding::default().top(PAGE_TOP).bottom(bottom).right(right)),
    )
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::new().width(4).margin(2).scroller_width(4),
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

/// A page's heading. Lives in the scroll with the rest of the page, so a
/// short window can give the rows the room instead of keeping a title parked
/// over them. The top inset is on `scroll`'s content, so every page starts
/// on the same line and that air is still there when you scroll back up.
fn heading<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    column![
        text(title).size(22).color(FG),
        Space::new().height(10),
        text(subtitle).size(13).color(MUTED),
        Space::new().height(SCROLL_PAD),
    ]
    .into()
}

/// Every settings screen is the same shape: a heading and a list that
/// scroll together, and a footer docked to the pane. The title yields its
/// room in a short window; the path and its action do not ride under the
/// last row.
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

    let body = column![heading(title, subtitle), list];

    match foot {
        // The footer sits below the scrollable, not inside it, so it never
        // gets `scroll_pad`'s own right inset - it needs its own, or it would
        // bleed to the window edge while everything above it stops short.
        //
        // The list keeps `ROW_PAD` under its last row, so the footer's
        // hairline gets the same air above it as every hairline between two
        // rows - it reads as the end of the list rather than as a second,
        // larger padding.
        //
        // Below it is a bar, not another row, and it is padded like one:
        // `FOOT_PAD` above and below its content, so the path and the button
        // sit centred in their band instead of tucked under the rule with the
        // page's full bottom margin left empty beneath them.
        Some(foot) => column![
            scroll_pad(body, ROW_PAD),
            container(hairline()).padding(iced::Padding::default().right(CONTENT_RIGHT)),
            container(foot).padding(
                iced::Padding::default().top(FOOT_PAD).bottom(FOOT_PAD).right(CONTENT_RIGHT),
            ),
        ]
        .height(Fill)
        .into(),
        None => scroll(body),
    }
}

/// Path (or save note) on the left, optional action on the right. The path
/// is the flexible half so a long directory cannot shove the button off.
fn path_cta<'a>(
    note: Element<'a, Message>,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut footer = row![container(note).width(Fill)];
    if let Some(action) = action {
        footer = footer.push(Space::new().width(12)).push(action);
    }
    footer.align_y(iced::Center).into()
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
                background: Some(Background::Color(mix(Color::TRANSPARENT, RAIL_ON, fill))),
                text_color: colour,
                border: Border { radius: 6.0.into(), ..Default::default() },
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
    .padding([ROW_PAD, 0.0])
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
            border: Border { color: EDGE, width: 1.0, radius: 10.0.into() },
            shadow: iced::Shadow {
                color: Color { a: 0.22, ..Color::BLACK },
                offset: iced::Vector::new(0.0, 2.0),
                blur_radius: 14.0,
            },
            ..Default::default()
        })
        .into()
}

/// A panel with its subject named along the top.
fn card<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    panel(column![text(title).size(12.5).color(MUTED), Space::new().height(12), content,].into())
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
/// instead of dividing by zero. Up takes the accent, down takes red.
fn trend(now: u32, before: u32) -> (String, Color) {
    if before == 0 {
        return if now == 0 {
            ("nothing yet".to_string(), MUTED)
        } else {
            ("first week with words".to_string(), ACCENT)
        };
    }
    let change = (now as i64 - before as i64) * 100 / before as i64;
    match change {
        0 => ("level with last week".to_string(), MUTED),
        up if up > 0 => (format!("+{up}% vs last week"), ACCENT),
        down => (format!("{down}% vs last week"), ERR),
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
            // Brighter than MUTED: the label above it is the quiet half of the
            // pair, and at the same weight neither reads as the answer.
            .color(mix(MUTED, FG, 0.4))
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

/// Like `clip`, but keeps the end. A path that does not fit should still name
/// the file; cutting from the start would leave a directory prefix and lose
/// the only part that distinguishes Config from History.
fn clip_tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    if count <= chars {
        return text.to_string();
    }
    let keep = chars.saturating_sub(1);
    let tail: String = text.chars().skip(count.saturating_sub(keep)).collect();
    format!("…{tail}")
}

/// `$HOME/…` as `~/…`, which is how the rest of Flow writes these paths.
/// Anything outside home is left alone - a custom XDG directory is the
/// actual location, not a tilde we invented.
fn display_path(path: &std::path::Path) -> String {
    collapse_home(path, std::env::var_os("HOME").as_deref().map(std::path::Path::new))
}

fn collapse_home(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    let shown = path.display().to_string();
    let Some(home) = home else {
        return shown;
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => shown,
    }
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
    days[..days.len() - 1].iter().rev().take_while(|day| day.words > 0).count()
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
    // The ramp stops short of the raw accent. A busy year fills this grid with
    // hundreds of cells, and at full strength that made the calendar the
    // loudest thing in the window - louder than the live pip, which is the one
    // place in the product the accent means something. The steps still read
    // apart from each other; they just do it below the accent's own weight.
    let t = (count as f32 / ceiling as f32).min(1.0);
    let step = if t > 0.75 {
        0.82
    } else if t > 0.5 {
        0.62
    } else if t > 0.25 {
        0.42
    } else {
        0.26
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
            border: Border { radius: (size * 0.22).into(), ..Default::default() },
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
        container(text(format!("{when} · {what}")).size(11.5).color(FG)).padding([5, 8]).style(
            |_theme| container::Style {
                background: Some(Background::Color(BG)),
                border: Border { color: mix(LINE, FG, 0.22), width: 1.0, radius: 6.0.into() },
                ..Default::default()
            },
        ),
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
                    _ => Space::new().width(Length::Fixed(CELL)).height(Length::Fixed(CELL)).into(),
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
            row![Space::new().width(Length::Fixed(WEEKDAY_GUTTER)), months,],
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
            row![text(caption).size(12).color(MUTED), Space::new().width(Fill), legend(),]
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
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
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
        update::Status::Installed(tag) => (OK, format!("{tag} installed - restart Flow to run it")),
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
    .padding([ROW_PAD, 0.0])
    .into()
}

/// Like `fact_row`, but the value is a path you can click to open in the
/// file manager. The type is the affordance; a second "Open" button would
/// repeat what the path already is.
///
/// The path is the shrinking half of the row: a long directory must not sit
/// on the label the way a short Session value can sit on a Fill. `~/…` is
/// what we draw; the real file is what a click reveals.
fn fact_path(label: &'static str, path: &std::path::Path) -> Element<'static, Message> {
    let real = path.to_path_buf();
    let shown = display_path(path);
    container(
        row![
            text(label).size(13.5).color(FG),
            Space::new().width(20),
            responsive(move |size| {
                // Default monospace at 12px is a little under 8px wide; the
                // extra room is the ellipsis, so a custom XDG path keeps the
                // filename instead of running off the pane.
                let chars = (size.width / 8.0).floor().max(8.0) as usize;
                container(path_link(real.clone(), clip_tail(&shown, chars)))
                    .width(Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .into()
            })
            .height(Length::Shrink),
        ]
        .align_y(iced::Center),
    )
    .padding([ROW_PAD, 0.0])
    .into()
}

fn path_link(path: std::path::PathBuf, shown: String) -> Element<'static, Message> {
    // A rich-text link, not a button: iced already turns those into a
    // pointer and an underline on hover, which is the affordance a path
    // sitting where Session's value sits would otherwise lack.
    rich_text![span(shown).size(12).font(Font::MONOSPACE).color(MUTED).link(path)]
        .on_link_click(Message::OpenPath)
        .wrapping(text::Wrapping::None)
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
                    text(if installed { "Installed" } else { "Missing" }).size(12).color(MUTED),
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
            border: Border { radius: 6.0.into(), ..Default::default() },
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
        border: Border { radius: 9.0.into(), ..Default::default() },
        ..Default::default()
    });

    button(track).padding(0).style(ghost).on_press(on_change(!value)).into()
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
        container(slider(range, value, on_change).height(14).style(|_theme, _status| {
            slider::Style {
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
            }
        }))
        .width(Length::Fixed(140.0)),
        Space::new().width(12),
        container(text(label.to_string()).size(12).font(Font::MONOSPACE).color(MUTED))
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
            border: Border { radius: 3.5.into(), ..Default::default() },
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

/// The slot copy sits in, on the meta line of a transcript. Fixed size at
/// every warmth so hovering never changes the row's height (see
/// `entry_row`); only the icon's opacity moves, using the row's own fade so
/// it settles in lockstep with the highlight rather than snapping in on top
/// of it.
///
/// A paper, not a word: drawn rather than picked from a font so it matches
/// the rest of the chrome instead of arriving as someone else's glyph. Copied
/// swaps the glyph itself to a checkmark, full opacity and accent - a colour
/// change alone on the same paper reads as "this button is now green", not as
/// "this worked".
///
/// A `mouse_area`, not a `button` and not a tooltip: both of those captured
/// the pointer event the parent row needs to stay hovered, and the tooltip
/// invalidated layout as it opened, which is what made the highlight drop
/// whenever the pointer reached the icon.
const COPY_SLOT: f32 = 20.0;
const COPY_GLYPH: f32 = 13.0;

fn copy_btn(index: usize, copied: bool, warmth: f32) -> Element<'static, Message> {
    let opacity = if copied { 1.0 } else { warmth };
    let colour = if copied { ACCENT } else { MUTED };
    iced::widget::mouse_area(
        container(
            Canvas::new(CopyMark { colour: Color { a: opacity, ..colour }, checked: copied })
                .width(Length::Fixed(COPY_GLYPH))
                .height(Length::Fixed(COPY_GLYPH)),
        )
        .width(Length::Fixed(COPY_SLOT))
        .height(Length::Fixed(COPY_SLOT))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center),
    )
    .on_press(Message::Copy(index))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

/// Two sheets, one offset, for "copy"; a single tick for "copied". Stroke
/// only, so it thins and fades with whatever colour and alpha it is asked
/// for rather than filling a blob that fights the rest of the row.
struct CopyMark {
    colour: Color,
    checked: bool,
}

impl canvas::Program<Message> for CopyMark {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let stroke = canvas::Stroke::default()
            .with_width(0.85)
            .with_color(self.colour)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round);

        // Built from the canvas's own size, as fractions, rather than points
        // tuned by eye for one fixed box: that's what let the two sheets sit
        // 1px low the first time round, and would do it again the next time
        // `COPY_GLYPH` changed. The sheets are placed by an equal-and-opposite
        // offset from the centre on each axis, which centres the combined
        // shape by construction - not by re-measuring its bounding box.
        let (w, h) = (bounds.width, bounds.height);
        let (cx, cy) = (w / 2.0, h / 2.0);

        if self.checked {
            let mut check = canvas::path::Builder::new();
            check.move_to(Point::new(0.127 * w, 0.527 * h));
            check.line_to(Point::new(0.382 * w, 0.800 * h));
            check.line_to(Point::new(0.873 * w, 0.200 * h));
            frame.stroke(&check.build(), stroke);
        } else {
            let (sw, sh) = (0.58 * w, 0.68 * h);
            let (dx, dy) = (0.12 * w, 0.12 * h);
            let radius = 0.12 * w.min(h);
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    Point::new(cx - sw / 2.0 + dx, cy - sh / 2.0 - dy),
                    Size::new(sw, sh),
                    radius.into(),
                ),
                stroke,
            );
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    Point::new(cx - sw / 2.0 - dx, cy - sh / 2.0 + dy),
                    Size::new(sw, sh),
                    radius.into(),
                ),
                stroke,
            );
        }
        vec![frame.into_geometry()]
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
        clip_tail, collapse_home, commas, current_streak, heat_ceiling, trend, Section, ACCENT,
        ERR, MUTED,
    };
    use std::path::Path;

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
        assert_eq!(trend(100, 100), ("level with last week".to_string(), MUTED));
        assert_eq!(trend(120, 100).1, ACCENT);
        assert_eq!(trend(80, 100).1, ERR);
        assert_eq!(trend(50, 0).1, ACCENT);
        assert_eq!(trend(0, 0), ("nothing yet".to_string(), MUTED));
    }

    #[test]
    fn long_numbers_stay_readable() {
        assert_eq!(commas(7), "7");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_000), "1,000");
        assert_eq!(commas(18_402), "18,402");
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
    fn home_is_written_as_a_tilde() {
        let home = Path::new("/home/j");
        assert_eq!(
            collapse_home(Path::new("/home/j/.config/flow/config.toml"), Some(home)),
            "~/.config/flow/config.toml"
        );
        assert_eq!(
            collapse_home(Path::new("/home/j/.local/share/flow/history.jsonl"), Some(home)),
            "~/.local/share/flow/history.jsonl"
        );
        assert_eq!(collapse_home(home, Some(home)), "~");
    }

    #[test]
    fn a_path_outside_home_is_left_alone() {
        assert_eq!(
            collapse_home(Path::new("/custom/config/flow/config.toml"), Some(Path::new("/home/j"))),
            "/custom/config/flow/config.toml"
        );
        assert_eq!(collapse_home(Path::new("/tmp/x"), None), "/tmp/x");
    }

    #[test]
    fn a_long_path_keeps_the_filename() {
        assert_eq!(clip_tail("abcdef", 6), "abcdef");
        assert_eq!(clip_tail("/private/tmp/claude/flow/config.toml", 17), "…flow/config.toml");
    }
}
