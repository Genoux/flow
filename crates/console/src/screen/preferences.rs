//! Everything you can change about how Flow behaves, on one page.
//!
//! Dictation and Audio used to be two rail sections holding four rows each,
//! which meant a click to find out whether the thing you wanted was on the
//! other one. They are groups now: the same rows, told apart by a label and
//! air, all reachable without navigating.
//!
//! Grouped by what a setting is about, which is the only thing that survives
//! being read by somebody who does not already know the app. "Dictation" was
//! the first attempt at a group name here and it carried nothing: this app only
//! does dictation, so the label was the product name rather than a topic, and
//! every row on the page would have qualified for it. GNOME's guidance names
//! "General" and "Options" as uninformative for exactly that reason. The fix is
//! to say what the rows have in common, not what the program is called.
//!
//! A row's title is the setting. The line under it is only written when the
//! title cannot carry the whole meaning on its own - which is most often not
//! the case, and never the place to name a systemd unit or the library doing
//! the work.
//!
//! General leads, as it does on every platform with a settings window, and holds
//! the app-level rows that belong to no topic - starting with the login item,
//! which is where macOS files its own.

use crate::card::panel_at;
use crate::control::{close_btn, option_row};
use crate::format::clip_tail;
use crate::theme::dissolve;
use crate::*;
use iced::widget::{column, mouse_area, row, stack, text, Space};
use iced::{Element, Font, Length};

/// The dialog's own measure. Wide enough for a full PipeWire description at
/// 13px, narrow enough that it still reads as a dialog on a 1040px window
/// rather than as a second page.
const DIALOG_WIDTH: f32 = 420.0;

impl Console {
    pub(super) fn preferences_section(&self) -> Element<'_, Message> {
        let body = column![
            group("General", self.general_rows()),
            group("Shortcut", self.shortcut_rows()),
            group("Microphone", self.microphone_rows()),
        ];

        // No subtitle. It was a table of contents for three group headings
        // already on screen a line below it.
        page_shell("Settings", "", body.into())
    }

    /// The app-level rows, which belong to no topic of their own. The login item
    /// is first: it is the row about the program rather than about dictating, so
    /// it is what somebody looks for before they have learned anything else.
    fn general_rows(&self) -> Vec<Element<'_, Message>> {
        let mut rows: Vec<Element<Message>> = Vec::new();
        // Only offered when systemd actually answered. A switch we cannot read
        // the true state of is worse than no switch.
        if let Some(enabled) = self.autostart {
            rows.push(setting(
                "Launch at login",
                "",
                toggle(enabled, self.travel("autostart"), Message::Autostart),
            ));
        }
        rows.push(setting(
            "Sounds",
            "Chime when dictation starts and stops.",
            toggle(self.settings.sound, self.travel("sound"), Message::Sound),
        ));
        rows
    }

    /// What starts and ends a dictation. "Chord" is what the daemon calls a
    /// key combination and it is musician's jargon out here - every product in
    /// this category says shortcut or hotkey, so the screen says Keys under a
    /// group called Shortcut.
    fn shortcut_rows(&self) -> Vec<Element<'_, Message>> {
        vec![
            setting(
                "Hold to talk",
                "Off: tap to start, tap to stop.",
                toggle(
                    self.settings.push_to_talk,
                    self.travel("push_to_talk"),
                    Message::PushToTalk,
                ),
            ),
            setting(
                "Keys",
                // No description: the value sitting beside this title is the
                // keys. The line that used to be here said so a second time,
                // and an older one before that sent people off to restart for
                // a rebinding that has been live by the next press ever since
                // `hotkey::spawn` started comparing the chord on every key.
                "",
                row![
                    text(if self.capturing {
                        "Press keys…".to_string()
                    } else {
                        self.settings.hotkey.replace('+', " ")
                    })
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(if self.capturing { ACCENT } else { MUTED }),
                    Space::new().width(12),
                    // Reset earns its place only when the chord is not already
                    // the default - offered next to a chord that is the default,
                    // it is a button that does nothing.
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
        ]
    }

    /// What Flow listens to, and what is done to the room before it does.
    /// Ducking belongs here rather than with the sounds Flow makes: it exists to
    /// keep your speakers out of the microphone, which is a fact about the
    /// input, not about the output.
    fn microphone_rows(&self) -> Vec<Element<'_, Message>> {
        vec![
            setting(
                "Input",
                self.input_hint(),
                action_msg("Change", false, Message::PickInput),
            ),
            setting(
                "Lower other apps",
                "",
                value_slider(
                    0..=100,
                    self.settings.duck,
                    Message::Duck,
                    &format!("{}%", self.settings.duck),
                ),
            ),
            setting(
                "Noise suppression",
                "Can muffle speech on some mics.",
                toggle(
                    self.settings.denoise,
                    self.travel("denoise"),
                    Message::Denoise,
                ),
            ),
        ]
    }

    /// The line under Input, which is the microphone itself.
    ///
    /// It used to explain the feature here - "Only Flow. Your system default is
    /// untouched." - while the device name sat in the value slot on the right,
    /// squeezed into a column shared with the "Lower other apps" slider and
    /// clipped at 26 characters because that is all that fits there. Two
    /// mistakes in one row: the fact belonged in the dialog, where it is read at
    /// the moment somebody is choosing rather than every time they scroll past,
    /// and the value belonged on the line that has the whole left column to
    /// spend. The dialog's subtitle says the fact now, and nothing is clipped.
    ///
    /// A value, so no full stop - the two lines are a label and its answer, not
    /// a sentence. The exceptions still read as answers: what Automatic
    /// resolves to is part of what Automatic currently means, and a machine with
    /// no microphone has to say so rather than name one.
    fn input_hint(&self) -> String {
        let Some(pinned) = self.settings.input_device.as_deref() else {
            return match (&self.input, self.sources.is_empty()) {
                (Some(default), _) => format!("Auto-detect · {default}"),
                (None, true) => "No microphone found".to_string(),
                (None, false) => {
                    "Auto-detect · your system default is not a microphone".to_string()
                }
            };
        };

        // The description, not the source name. `..._USB_Audio-00.HiFi_5_1__Mic\
        // __source` is what the config stores; "USB Audio Microphone" is what a
        // person picked. The raw name only surfaces for a microphone that is
        // pinned and not in the graph, where there is no description to be had -
        // clipped from the end, because its tail is what tells one port on a
        // card from another.
        self.sources
            .iter()
            .find(|(name, _)| name == pinned)
            .map(|(_, description)| description.clone())
            .unwrap_or_else(|| format!("{} · not connected", clip_tail(pinned, 34)))
    }

    /// The microphone dialog, or nothing while it is shut.
    ///
    /// A dialog rather than a menu because the choice needs more than a label
    /// per row. Automatic is not a device and has to say where it takes its
    /// answer from; a pinned microphone that is switched off has to stay
    /// offerable and say that it is off. Neither fits in a `pick_list` option,
    /// and both are the difference between a list of names and a list you can
    /// choose from.
    ///
    /// Stacked over the console by `view` on the same `inert` + veil pattern
    /// setup uses, so there is one way in this window for something to sit on
    /// top of something else.
    pub(crate) fn mic_dialog(&self) -> Option<Element<'_, Message>> {
        let picker = self.picking_input?;
        // Nothing left to draw: the fade has run out and `Tick` is about to drop
        // the state. Drawing it at lift 0 would leave an invisible veil eating
        // clicks meant for the page underneath.
        if picker.spent(self.now) {
            return None;
        }
        let lift = picker.lift(self.now);

        let mut rows = column![option_row(
            // Where it takes its answer from, in the row's own name, so the
            // list is one line per choice. Not the microphone it resolves to
            // today: that read as settled when it is only a reading, and it put
            // the same string on two rows stacked one above the other.
            match &self.input {
                Some(_) => "Auto-detect (system default)",
                None => "Auto-detect (no system default)",
            },
            String::new(),
            self.settings.input_device.is_none(),
            lift,
            Message::InputDevice(None),
        )]
        .spacing(2);

        for (name, description) in &self.sources {
            rows = rows.push(option_row(
                description,
                String::new(),
                self.settings.input_device.as_deref() == Some(name.as_str()),
                lift,
                Message::InputDevice(Some(name.clone())),
            ));
        }

        // A pinned microphone that is not in the graph right now - unplugged, or
        // a headset that is off. Offered anyway: dropping it would leave the row
        // naming a device the dialog denied existed, and no way back to it once
        // the list had forgotten it. Clipped from the end, because this is the
        // raw source name and its tail is what separates `..._Mic__source` from
        // `..._Line__source`.
        if let Some(missing) = self
            .settings
            .input_device
            .as_deref()
            .filter(|name| !self.sources.iter().any(|(id, _)| id == name))
        {
            rows = rows.push(option_row(
                &clip_tail(missing, 34),
                "Not connected".to_string(),
                true,
                lift,
                Message::InputDevice(Some(missing.to_owned())),
            ));
        }

        let dialog = panel_at(
            lift,
            column![
                // Indented by the row buttons' own horizontal padding, so the
                // title and the option labels share a left edge. Without it the
                // header hangs 11px left of the list it introduces. Left only:
                // the row is `Fill`, so the close button keeps the corner.
                iced::widget::container(column![
                    row![
                        text("Microphone").size(15).color(dissolve(FG, lift)),
                        Space::new().width(Length::Fill),
                        close_btn(lift),
                    ]
                    .align_y(iced::Center),
                    Space::new().height(4),
                    text("Flow records from this. Your system default stays as it is.")
                        .size(12)
                        .color(dissolve(FAINT, lift)),
                ])
                .padding(iced::Padding::ZERO.left(11)),
                Space::new().height(14),
                rows,
            ]
            .into(),
        );

        Some(
            stack![
                // Click-off, which is the other half of a close button. The
                // dialog is above this in the stack, so a click that lands on
                // the dialog never reaches it.
                //
                // Black, not `BG`. Setup's veil is `BG` because it covers the
                // whole window and becomes the new ground - but a scrim's job
                // is to darken the old one, and `BG` over a page whose ground
                // is already `BG` darkens it by ΔL* 0.00. It dimmed the text
                // and the switches and left the ground exactly where it was,
                // which is why the dialog read as washed out rather than
                // floating: it had only ΔL* 4.66 of separation from a scrim
                // that had not moved. Black at 0.66 takes the ground to
                // #030405 and the panel's separation to ΔL* 6.63.
                mouse_area(
                    iced::widget::container(Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(move |_| iced::widget::container::Style {
                            background: Some(iced::Background::Color(iced::Color {
                                a: 0.66 * lift,
                                ..iced::Color::BLACK
                            })),
                            ..Default::default()
                        })
                )
                .on_press(Message::ClosePicker),
                iced::widget::container(
                    iced::widget::container(dialog).max_width(DIALOG_WIDTH)
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Center),
            ]
            .into(),
        )
    }
}
