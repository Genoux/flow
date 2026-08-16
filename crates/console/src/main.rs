//! Flow's status and settings window.
//!
//! A separate binary from the daemon on purpose: iced brings wgpu with it, and
//! the daemon has no business carrying that to record audio. The two will talk
//! over the existing ipc socket - every value here is still mock, so the whole
//! window can be navigated and judged before any of it is wired up.

mod daemon;

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

fn subscription(_state: &Console) -> Subscription<Message> {
    Subscription::run(|| iced::futures::StreamExt::map(daemon::stream(), Message::Daemon))
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
    Status,
    Dictation,
    Audio,
    Models,
    About,
}

impl Section {
    const ALL: [Section; 5] = [
        Section::Status,
        Section::Dictation,
        Section::Audio,
        Section::Models,
        Section::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::Status => "Status",
            Section::Dictation => "Dictation",
            Section::Audio => "Audio",
            Section::Models => "Models",
            Section::About => "About",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    SystemDefault,
    Webcam,
    Headset,
}

impl Input {
    const ALL: [Input; 3] = [Input::SystemDefault, Input::Webcam, Input::Headset];
}

impl std::fmt::Display for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Input::SystemDefault => "System default",
            Input::Webcam => "Full HD webcam",
            Input::Headset => "USB headset",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gpu {
    Auto,
    Discrete,
    Cpu,
}

impl Gpu {
    const ALL: [Gpu; 3] = [Gpu::Auto, Gpu::Discrete, Gpu::Cpu];
}

impl std::fmt::Display for Gpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Gpu::Auto => "Automatic",
            Gpu::Discrete => "RTX 3060 Ti",
            Gpu::Cpu => "CPU only",
        })
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
    SetInput(Input),
    SetGpu(Gpu),
    Daemon(daemon::Event),
    Noop,
}

struct Console {
    section: Section,
    daemon: daemon::State,
    push_to_talk: bool,
    cleanup: bool,
    terminal: bool,
    denoise: bool,
    autostart: bool,
    duck: u32,
    settle: u32,
    input: Input,
    gpu: Gpu,
}

impl Console {
    fn new() -> (Self, Task<Message>) {
        (
            Self {
                section: Section::Status,
                daemon: daemon::State::default(),
                push_to_talk: true,
                cleanup: true,
                terminal: false,
                denoise: false,
                autostart: true,
                duck: 50,
                settle: 150,
                input: Input::SystemDefault,
                gpu: Gpu::Auto,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Select(section) => self.section = section,
            Message::PushToTalk(on) => self.push_to_talk = on,
            Message::Cleanup(on) => self.cleanup = on,
            Message::Terminal(on) => self.terminal = on,
            Message::Denoise(on) => self.denoise = on,
            Message::Autostart(on) => self.autostart = on,
            Message::Duck(value) => self.duck = value,
            Message::Settle(value) => self.settle = value,
            Message::SetInput(input) => self.input = input,
            Message::SetGpu(gpu) => self.gpu = gpu,
            Message::Daemon(daemon::Event::Line(line)) => self.daemon.apply(&line),
            Message::Daemon(daemon::Event::Disconnected) => {
                self.daemon = daemon::State::default()
            }
            Message::Noop => {}
        }
        Task::none()
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
                text("0.3.1").size(11).font(Font::MONOSPACE).color(FAINT),
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
            Section::Status => self.status_section(),
            Section::Dictation => self.dictation_section(),
            Section::Audio => self.audio_section(),
            Section::Models => self.models_section(),
            Section::About => self.about_section(),
        };

        container(content)
            .width(Fill)
            .height(Fill)
            .padding([34, 36])
            .into()
    }

    fn status_section(&self) -> Element<'_, Message> {
        use daemon::Activity;

        // A problem the daemon reported outranks whatever it is nominally
        // doing: the user cannot act on "ready" if the last paste failed.
        let (pip_colour, heading, sub) = match (&self.daemon.problem, self.daemon.activity) {
            (Some(problem), _) => (ERR, "Something went wrong", problem.as_str()),
            (None, Activity::Offline) => (
                FAINT,
                "Flow isn't running",
                "Start the daemon and this window will connect on its own.",
            ),
            (None, Activity::Starting) => (
                ACCENT,
                "Starting",
                "Loading the speech and cleanup models. This happens once.",
            ),
            (None, Activity::Ready) => (
                FAINT,
                "Ready",
                "Hold the chord anywhere. Flow types into whatever has focus.",
            ),
            (None, Activity::Listening) => (
                ACCENT,
                "Listening",
                "Other apps are turned down. Release to transcribe.",
            ),
            (None, Activity::Working) => (
                ACCENT,
                "Transcribing",
                "Working on what you just said.",
            ),
        };

        let last = self
            .daemon
            .recent
            .first()
            .map(|d| format!("{} ms", d.paste_ms))
            .unwrap_or_else(|| "-".to_string());
        let words = self.daemon.words.to_string();

        let mut body = column![
            row![pip(pip_colour), text(heading).size(22).color(FG)]
                .spacing(10)
                .align_y(iced::Center),
            Space::new().height(10),
            text(sub).size(13).color(MUTED),
            Space::new().height(30),
        ];

        if self.daemon.activity == Activity::Offline {
            body = body.push(
                text("Nothing to show until the daemon is up.")
                    .size(12)
                    .color(FAINT),
            );
        } else {
            body = body.push(recent(&self.daemon.recent));
            body = body.push(Space::new().height(26));
            body = body.push(facts_row(&[
                ("last paste", last.as_str()),
                ("words", words.as_str()),
                ("model", "parakeet-tdt"),
            ]));
        }

        column![
            body,
            Space::new().height(Fill),
            row![
                text("Hold super shift d").size(12).color(FAINT),
                Space::new().width(Fill),
                action(
                    if self.daemon.activity == Activity::Offline {
                        "Start"
                    } else {
                        "Stop"
                    },
                    false
                ),
            ]
            .align_y(iced::Center),
        ]
        .into()
    }

    fn dictation_section(&self) -> Element<'_, Message> {
        let rows: Vec<Element<Message>> = vec![
            setting(
                "Push to talk",
                "Flow watches the chord itself, so no compositor binding is needed.",
                toggle(self.push_to_talk, Message::PushToTalk),
            ),
            setting(
                "Chord",
                "Held down while you speak.",
                row![
                    text("super shift d").size(12).font(Font::MONOSPACE).color(MUTED),
                    Space::new().width(12),
                    action("Change", false),
                ]
                .align_y(iced::Center)
                .into(),
            ),
            setting(
                "Clean up transcript",
                "Removes filler and fixes punctuation with the local model.",
                toggle(self.cleanup, Message::Cleanup),
            ),
            setting(
                "Terminal paste chord",
                "Send Ctrl+Shift+V when a terminal has focus.",
                toggle(self.terminal, Message::Terminal),
            ),
            setting(
                "Vocabulary",
                "Names and jargon the recogniser should get right.",
                row![
                    text("12 terms").size(12).font(Font::MONOSPACE).color(MUTED),
                    Space::new().width(12),
                    action("Edit", false),
                ]
                .align_y(iced::Center)
                .into(),
            ),
            setting(
                "Start with session",
                "Launch the daemon when you log in.",
                toggle(self.autostart, Message::Autostart),
            ),
        ];

        section_shell("Dictation", "How the chord behaves and what happens to your words.", rows, None)
    }

    fn audio_section(&self) -> Element<'_, Message> {
        let rows: Vec<Element<Message>> = vec![
            setting(
                "Input",
                "Whichever source PipeWire is sending Flow.",
                picker(&Input::ALL[..], self.input, Message::SetInput),
            ),
            setting(
                "Turn other apps down",
                "Keeps your speakers out of the microphone while you dictate.",
                value_slider(0..=100, self.duck, Message::Duck, &format!("{}%", self.duck)),
            ),
            setting(
                "Settle before recording",
                "Wait for the volume to actually drop before the mic opens.",
                value_slider(0..=600, self.settle, Message::Settle, &format!("{} ms", self.settle)),
            ),
            setting(
                "Noise suppression",
                "Runs RNNoise over the audio. Can blunt consonants on a weak mic.",
                toggle(self.denoise, Message::Denoise),
            ),
        ];

        section_shell(
            "Audio",
            "What Flow listens to, and what it does to the room first.",
            rows,
            None,
        )
    }

    fn models_section(&self) -> Element<'_, Message> {
        let rows: Vec<Element<Message>> = vec![
            model_row("Speech", "parakeet-tdt-0.6b · int8", "620 MB", true),
            model_row("Cleanup", "qwen3-1.7b · Q4_K_M", "1.1 GB", true),
            setting(
                "Run cleanup on",
                "Speech recognition always runs on the CPU.",
                picker(&Gpu::ALL[..], self.gpu, Message::SetGpu),
            ),
        ];

        section_shell(
            "Models",
            "Both models run on this machine. Nothing you say leaves it.",
            rows,
            Some(
                row![
                    text("1.7 GB in ~/.local/share/flow")
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                    Space::new().width(Fill),
                    action("Check for updates", false),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        )
    }

    fn about_section(&self) -> Element<'_, Message> {
        let rows: Vec<Element<Message>> = vec![
            fact_row("Version", "0.3.1"),
            fact_row("Config", "~/.config/flow/config.toml"),
            fact_row("Models", "~/.local/share/flow/models"),
            fact_row("Compositor", "Hyprland · Wayland"),
            fact_row("Licence", "MIT"),
        ];

        section_shell(
            "Flow",
            "Push-to-talk dictation that runs entirely on your own machine.",
            rows,
            Some(
                row![
                    text("Up to date").size(12).color(FAINT),
                    Space::new().width(Fill),
                    action("Open config", false),
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
        scrollable(list).height(Fill),
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
fn fact_row<'a>(label: &'a str, value: &'a str) -> Element<'a, Message> {
    container(
        row![
            text(label).size(13.5).color(FG),
            Space::new().width(Fill),
            text(value).size(12).font(Font::MONOSPACE).color(MUTED),
        ]
        .align_y(iced::Center),
    )
    .padding([14, 0])
    .into()
}

fn model_row<'a>(
    label: &'a str,
    detail: &'a str,
    size: &'a str,
    installed: bool,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(label).size(13.5).color(FG),
                Space::new().height(3),
                text(detail).size(12).font(Font::MONOSPACE).color(FAINT),
            ]
            .width(Length::FillPortion(3)),
            Space::new().width(20),
            row![
                text(size).size(12).font(Font::MONOSPACE).color(FAINT),
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

/// Rows separated by a hairline, with no box around them - the container was
/// furniture, the separation is the only part that carried meaning.
fn recent(entries: &[daemon::Dictation]) -> Element<'_, Message> {
    let mut list = column![text("Recent").size(12).color(FAINT), Space::new().height(4)];

    if entries.is_empty() {
        return list
            .push(
                container(text("Nothing dictated yet.").size(13).color(FAINT)).padding([8, 0]),
            )
            .into();
    }

    // Three is what the pane has room for without scrolling; the daemon keeps
    // more than that, and the rest belong in a history view rather than here.
    for (index, entry) in entries.iter().take(3).enumerate() {
        list = list.push(
            container(
                row![
                    text(entry.text.clone()).size(13).color(FG),
                    Space::new().width(Fill),
                    text(format!("{:.1}s", entry.spoken))
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                ]
                .align_y(iced::Center),
            )
            .padding([8, 0]),
        );
        if index + 1 < entries.len().min(3) {
            list = list.push(hairline());
        }
    }

    list.into()
}

/// Inline key/value pairs. Boxing three numbers in bordered tiles was the
/// dashboard reflex this design is trying not to have.
fn facts_row(pairs: &[(&str, &str)]) -> Element<'static, Message> {
    let mut line = row![].spacing(26);
    for (key, value) in pairs {
        line = line.push(
            row![
                text(key.to_string()).size(12).font(Font::MONOSPACE).color(FAINT),
                Space::new().width(6),
                text(value.to_string()).size(12).font(Font::MONOSPACE).color(MUTED),
            ]
            .align_y(iced::Center),
        );
    }
    line.into()
}

fn action(label: &str, primary: bool) -> Element<'static, Message> {
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
    .on_press(Message::Noop)
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
