//! Flow's status and settings window.
//!
//! A separate binary from the daemon on purpose: iced brings wgpu with it, and
//! the daemon has no business carrying that to record audio. The two will talk
//! over the existing ipc socket - every value here is still mock, so the whole
//! window can be navigated and judged before any of it is wired up.

mod daemon;
mod history;
mod settings;
mod system;
mod vocabulary;

use iced::widget::{button, column, container, pick_list, row, scrollable, slider, text, toggler, Space};
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
const ACCENT: Color = Color { r: 0.847, g: 0.651, b: 0.341, a: 1.0 }; // #D8A657
const ERR: Color = Color { r: 0.831, g: 0.451, b: 0.420, a: 1.0 }; // #D4736B
const ON_ACCENT: Color = Color { r: 0.078, g: 0.082, b: 0.059, a: 1.0 };

const RAIL_WIDTH: f32 = 176.0;

fn main() -> iced::Result {
    iced::application(Console::new, Console::update, Console::view)
        .title("Flow")
        .theme(theme)
        .subscription(subscription)
        .window(iced::window::Settings {
            size: iced::Size::new(880.0, 580.0),
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

fn subscription(state: &Console) -> Subscription<Message> {
    let daemon = Subscription::run(|| iced::futures::StreamExt::map(daemon::stream(), Message::Daemon));
    if !state.capturing {
        return daemon;
    }
    // Only while capturing: a settings window that swallows every keystroke
    // the rest of the time would be its own bug.
    Subscription::batch([
        daemon,
        iced::keyboard::listen().map(|event| match event {
            iced::keyboard::Event::KeyPressed {
                key, modifiers, ..
            } => Message::Captured(key, modifiers),
            _ => Message::Noop,
        }),
    ])
}

/// Turn a captured keypress into the daemon's chord spelling, or `None` when
/// it is not a usable binding.
///
/// Rejects a bare key and a lone modifier deliberately: push-to-talk means
/// holding something, and the daemon refuses a chord whose trigger is itself a
/// modifier because it could never complete.
fn chord_from(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<String> {
    let mut parts = Vec::new();
    if modifiers.logo() {
        parts.push("super".to_string());
    }
    if modifiers.control() {
        parts.push("ctrl".to_string());
    }
    if modifiers.alt() {
        parts.push("alt".to_string());
    }
    if modifiers.shift() {
        parts.push("shift".to_string());
    }
    if parts.is_empty() {
        return None;
    }

    let trigger = match key {
        iced::keyboard::Key::Character(c) => {
            let c = c.trim().to_lowercase();
            // One character, and not punctuation the daemon has no keycode for.
            (c.len() == 1 && c.chars().all(|ch| ch.is_ascii_alphanumeric())).then_some(c)?
        }
        iced::keyboard::Key::Named(named) => named_trigger(*named)?,
        _ => return None,
    };

    parts.push(trigger);
    Some(parts.join("+"))
}

/// The named keys worth binding. Anything not here is left out rather than
/// guessed at: a chord the daemon cannot parse would be written to the config
/// and only fail at its next startup.
fn named_trigger(named: iced::keyboard::key::Named) -> Option<String> {
    use iced::keyboard::key::Named;
    Some(
        match named {
            Named::Space => "space",
            Named::Enter => "enter",
            Named::Tab => "tab",
            Named::F1 => "f1",
            Named::F2 => "f2",
            Named::F3 => "f3",
            Named::F4 => "f4",
            Named::F5 => "f5",
            Named::F6 => "f6",
            Named::F7 => "f7",
            Named::F8 => "f8",
            Named::F9 => "f9",
            Named::F10 => "f10",
            Named::F11 => "f11",
            Named::F12 => "f12",
            _ => return None,
        }
        .to_string(),
    )
}

fn style(_state: &Console, _theme: &Theme) -> iced::theme::Style {
    iced::theme::Style {
        background_color: BG,
        text_color: FG,
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    History,
    Dictation,
    Audio,
    Vocabulary,
    Models,
    About,
}

impl Section {
    const ALL: [Section; 6] = [
        Section::History,
        Section::Dictation,
        Section::Audio,
        Section::Vocabulary,
        Section::Models,
        Section::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::History => "History",
            Section::Dictation => "Dictation",
            Section::Audio => "Audio",
            Section::Vocabulary => "Vocabulary",
            Section::Models => "Models",
            Section::About => "About",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gpu {
    Auto,
    Device(u32),
}

impl Gpu {
    const ALL: [Gpu; 3] = [Gpu::Auto, Gpu::Device(0), Gpu::Device(1)];

    fn from_config(index: Option<u32>) -> Self {
        match index {
            None => Gpu::Auto,
            Some(index) => Gpu::Device(index),
        }
    }

    fn to_config(self) -> Option<u32> {
        match self {
            Gpu::Auto => None,
            Gpu::Device(index) => Some(index),
        }
    }
}

impl std::fmt::Display for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gpu::Auto => f.write_str("Automatic"),
            Gpu::Device(index) => write!(f, "Device {index}"),
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
    Settle(u32),
    SetGpu(Gpu),
    OpenConfig,
    OpenVocabulary,
    /// Start listening for the next chord the user presses.
    CaptureChord,
    /// A key arrived while capturing.
    Captured(iced::keyboard::Key, iced::keyboard::Modifiers),
    CancelCapture,
    TypingTerm(String),
    AddTerm,
    RemoveTerm(usize),
    Daemon(daemon::Event),
    Noop,
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
    models: Vec<system::Model>,
    session: String,
    /// True while waiting for the user to press a new chord.
    terms: Vec<String>,
    typing: String,
    term_error: Option<String>,
    capturing: bool,
    /// Why the last attempted chord was rejected, shown in place of the hint.
    chord_error: Option<String>,
}

impl Console {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                section: Section::History,
                daemon: daemon::State::default(),
                settings: settings::Settings::load(),
                save_error: None,
                saved: false,
                autostart: system::autostart_enabled(),
                input: system::default_input(),
                entries: history::recent(),
                models: system::models(),
                session: system::session(),
                terms: vocabulary::load(),
                typing: String::new(),
                term_error: None,
                capturing: false,
                chord_error: None,
            },
            Task::none(),
        )
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
            Message::PushToTalk(on) => {
                self.settings.push_to_talk = on;
                self.persist();
            }
            Message::Cleanup(on) => {
                self.settings.cleanup = on;
                self.persist();
            }
            Message::Terminal(on) => {
                self.settings.terminal = on;
                self.persist();
            }
            Message::Denoise(on) => {
                self.settings.denoise = on;
                self.persist();
            }
            Message::Duck(value) => {
                self.settings.duck = value;
                self.persist();
            }
            Message::Settle(value) => {
                self.settings.duck_settle_ms = value as u64;
                self.persist();
            }
            Message::Autostart(on) => match system::set_autostart(on) {
                // Re-read rather than assume: systemd is the authority on
                // whether that worked, not our optimism.
                Ok(()) => {
                    self.autostart = system::autostart_enabled();
                    self.save_error = None;
                }
                Err(err) => self.save_error = Some(err),
            },
            Message::SetGpu(gpu) => {
                self.settings.gpu = gpu.to_config();
                self.persist();
            }
            Message::Daemon(daemon::Event::Line(line)) => {
                let before = self.daemon.words;
                self.daemon.apply(&line);
                // Re-read the file rather than trust the socket's copy: the
                // file is what this window shows, and it is the thing that
                // outlives the daemon.
                if self.daemon.words != before {
                    self.entries = history::recent();
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
            }
            Message::CancelCapture => self.capturing = false,
            Message::Captured(key, modifiers) => {
                if let Some(chord) = chord_from(&key, modifiers) {
                    self.capturing = false;
                    self.settings.hotkey = chord;
                    self.persist();
                } else if matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                ) {
                    self.capturing = false;
                } else {
                    // A bare key, or a modifier on its own. Say why and keep
                    // listening rather than saving something the daemon would
                    // refuse at startup.
                    self.chord_error =
                        Some("Hold at least one modifier, then a letter or number.".into());
                }
            }
            Message::OpenConfig => {
                if let Err(err) = system::open(&settings::config_path()) {
                    self.save_error = Some(err);
                }
            }
            Message::OpenVocabulary => {
                let path = settings::config_path()
                    .parent()
                    .map(|dir| dir.join("vocabulary.txt"))
                    .unwrap_or_default();
                if let Err(err) = system::open(&path) {
                    self.save_error = Some(err);
                }
            }
            Message::Noop => {}
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
        let mut items = column![text("Flow").size(14).color(FG), Space::new().height(16)].spacing(11);

        for section in Section::ALL {
            items = items.push(nav(section, section == self.section));
        }

        container(
            column![
                items,
                Space::new().height(Fill),
                text(env!("CARGO_PKG_VERSION")).size(11).font(Font::MONOSPACE).color(FAINT),
            ]
            .spacing(0),
        )
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Fill)
        .padding([26, 20])
        .into()
    }

    // -- pane ---------------------------------------------------------------

    fn pane(&self) -> Element<'_, Message> {
        let content = match self.section {
            Section::History => self.history_section(),
            Section::Dictation => self.dictation_section(),
            Section::Audio => self.audio_section(),
            Section::Vocabulary => self.vocabulary_section(),
            Section::Models => self.models_section(),
            Section::About => self.about_section(),
        };

        container(content)
            .width(Fill)
            .height(Fill)
            .padding([34, 36])
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
                            text(entry.text.clone()).size(13).color(FG),
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

        let running = self.daemon.activity != daemon::Activity::Offline;
        let foot = row![
            pip(if running { ACCENT } else { FAINT }),
            Space::new().width(8),
            text(if running {
                "Flow is running"
            } else {
                "Flow isn't running"
            })
            .size(12)
            .color(FAINT),
            Space::new().width(Fill),
            text(format!("{} kept", self.entries.len()))
                .size(12)
                .font(Font::MONOSPACE)
                .color(FAINT),
        ]
        .align_y(iced::Center);

        column![
            text("History").size(22).color(FG),
            Space::new().height(10),
            text("Everything Flow has typed for you, most recent first.")
                .size(13)
                .color(MUTED),
            Space::new().height(26),
            scrollable(container(list).padding(iced::Padding::default().right(16))).height(Fill),
            Space::new().height(16),
            hairline(),
            Space::new().height(16),
            foot,
        ]
        .into()
    }

    fn dictation_section(&self) -> Element<'_, Message> {
        let mut rows: Vec<Element<Message>> = vec![
            setting(
                "Push to talk",
                "Flow watches the chord itself, so no compositor binding is needed. Takes effect when Flow restarts.",
                toggle(self.settings.push_to_talk, Message::PushToTalk),
            ),
            setting(
                "Chord",
                "Held down while you speak. Takes effect when Flow restarts.",
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
                    } else {
                        action_msg("Change", false, Message::CaptureChord)
                    },
                ]
                .align_y(iced::Center)
                .into(),
            ),
            setting(
                "Clean up transcript",
                "Removes filler and fixes punctuation with the local model. Turning it back on needs a restart.",
                toggle(self.settings.cleanup, Message::Cleanup),
            ),
            setting(
                "Terminal paste chord",
                "Send Ctrl+Shift+V when a terminal has focus.",
                toggle(self.settings.terminal, Message::Terminal),
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
                toggle(enabled, Message::Autostart),
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
        let mut rows: Vec<Element<Message>> = vec![
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
                "Settle before recording",
                "Wait for the volume to actually drop before the mic opens.",
                value_slider(0..=600, self.settings.duck_settle_ms as u32, Message::Settle, &format!("{} ms", self.settings.duck_settle_ms)),
            ),
            setting(
                "Noise suppression",
                "Runs RNNoise over the audio. Can blunt consonants on a weak mic.",
                toggle(self.settings.denoise, Message::Denoise),
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
            scrollable(container(list).padding(iced::Padding::default().right(16))).height(Fill),
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

        let mut rows: Vec<Element<Message>> = self
            .models
            .iter()
            .zip(&sizes)
            .map(|(model, size)| {
                model_row(model.label, model.detail.clone(), size.clone(), model.installed)
            })
            .collect();

        rows.push(setting(
                "Run cleanup on",
                "Speech recognition always runs on the CPU. Takes effect when Flow restarts.",
                picker(&Gpu::ALL[..], Gpu::from_config(self.settings.gpu), Message::SetGpu),
        ));

        let total = format!(
            "{} in {}",
            system::human_bytes(self.models.iter().map(|m| m.bytes).sum()),
            system::data_home().join("flow/models").display()
        );

        section_shell(
            "Models",
            "Both models run on this machine. Nothing you say leaves it.",
            rows,
            Some(
                row![
                    text(total).size(12)
                    .font(Font::MONOSPACE)
                    .color(FAINT),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        )
    }

    fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        let config = settings::config_path().display().to_string();
        let history_file = history::path().display().to_string();
        let rows: Vec<Element<Message>> = vec![
            fact_row("Version", env!("CARGO_PKG_VERSION")),
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
}

// ---------------------------------------------------------------------------
// Shells
// ---------------------------------------------------------------------------

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
        scrollable(container(list).padding(iced::Padding::default().right(16))).height(Fill),
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

fn nav(section: Section, selected: bool) -> Element<'static, Message> {
    button(
        text(section.label())
            .size(13)
            .color(if selected { FG } else { FAINT }),
    )
    .padding(0)
    .style(ghost)
    .on_press(Message::Select(section))
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
            row![
                text(size.into()).size(12).font(Font::MONOSPACE).color(FAINT),
                Space::new().width(14),
                pip(if installed { FAINT } else { ACCENT }),
                Space::new().width(7),
                text(if installed { "Installed" } else { "Missing" })
                    .size(12)
                    .color(MUTED),
            ]
            .align_y(iced::Center)
            .width(Length::FillPortion(2)),
        ]
        .align_y(iced::Center),
    )
    .padding([14, 0])
    .into()
}

fn toggle(value: bool, on_change: fn(bool) -> Message) -> Element<'static, Message> {
    toggler(value)
        .on_toggle(on_change)
        .size(18)
        .style(|_theme, status| {
            let on = matches!(
                status,
                toggler::Status::Active { is_toggled: true }
                    | toggler::Status::Hovered { is_toggled: true }
            );
            toggler::Style {
                background: Background::Color(if on { ACCENT } else { LINE }),
                background_border_width: 0.0,
                background_border_color: Color::TRANSPARENT,
                foreground: Background::Color(if on { ON_ACCENT } else { MUTED }),
                foreground_border_width: 0.0,
                foreground_border_color: Color::TRANSPARENT,
                text_color: None,
                border_radius: None,
                padding_ratio: 0.2,
            }
        })
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

fn picker<'a, T>(options: &'a [T], selected: T, on_select: fn(T) -> Message) -> Element<'a, Message>
where
    T: ToString + PartialEq + Clone + 'a,
{
    pick_list(options, Some(selected), on_select)
        .text_size(12)
        .padding([6, 10])
        .style(|_theme, _status| pick_list::Style {
            text_color: FG,
            placeholder_color: FAINT,
            handle_color: FAINT,
            background: Background::Color(BG),
            border: Border {
                color: LINE,
                width: 1.0,
                radius: 6.0.into(),
            },
        })
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
            background: primary.then(|| Background::Color(ACCENT)),
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
