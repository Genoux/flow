//! Input device, ducking, and denoise.

use crate::*;
use iced::widget::text;
use iced::{Element, Font};

impl Console {
    pub(super) fn audio_section(&self) -> Element<'_, Message> {
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
}
