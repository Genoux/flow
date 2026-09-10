use super::editorial;
use crate::*;
use iced::widget::{column, container, responsive, row, text, Space};
use iced::{Background, Border, Color, Element, Fill};

impl Console {
    pub(super) fn overview_section(&self) -> Element<'_, Message> {
        responsive(move |size| self.overview_content(size.width)).into()
    }

    fn overview_content(&self, width: f32) -> Element<'_, Message> {
        let wide = width >= 650.0;
        let status = status_of(self.incomplete(), self.daemon.activity);
        let needs_setup = status == Status::NeedsSetup;
        let running = status == Status::Running;
        let (label, dot) = if needs_setup {
            ("Setup unfinished", ERR)
        } else {
            activity_label(self.daemon.activity, self.daemon.reachable)
        };
        let mut header = row![
            text("Overview").size(22).color(FG),
            Space::new().width(Fill),
            pip(dot),
            Space::new().width(9),
            text(label).size(12).color(dot),
        ]
        .align_y(iced::Center);
        if !needs_setup {
            header = header.push(Space::new().width(16)).push(action_msg(
                service_action_label(running),
                !running,
                self.service_pending
                    .is_none()
                    .then_some(Message::Service(if running { "stop" } else { "start" })),
            ));
        }
        let total = self.days.len();
        let this_week = &self.days[total - 7..];
        let last_week = &self.days[total - 14..total - 7];
        let spoken: f32 = this_week.iter().map(|day| day.spoken).sum();
        let dictations: u32 = this_week.iter().map(|day| day.dictations).sum();
        let active = this_week.iter().filter(|day| day.dictations > 0).count();
        let words = crate::history::words(this_week);
        let comparison = trend(words, crate::history::words(last_week));
        let introduction =
            editorial::summary_banner(commas(words), comparison.0, width - CONTENT_RIGHT);
        let count = stat_tile(
            "Dictations",
            dictations.to_string(),
            (format!("{active} of 7 days"), MUTED),
        );
        let time = stat_tile(
            "Speaking time",
            crate::history::duration(spoken),
            (
                if dictations == 0 {
                    "nothing this week".to_owned()
                } else {
                    format!(
                        "{} average",
                        crate::history::duration(spoken / dictations as f32)
                    )
                },
                MUTED,
            ),
        );
        let streak = stat_tile(
            "Current streak",
            plural(current_streak(&self.days) as u32, "day"),
            (
                format!(
                    "longest {}",
                    plural(longest_streak(&self.days) as u32, "day")
                ),
                MUTED,
            ),
        );
        let metrics: Element<'_, Message> = if wide {
            row![count, time, streak].spacing(24).into()
        } else {
            column![row![count, time].spacing(24), streak]
                .spacing(24)
                .into()
        };
        let mut page = column![header, Space::new().height(26)];
        if let Some(banner) = self.install_banner() {
            page = page.push(banner).push(Space::new().height(20));
        }
        for (colour, note) in self.attention() {
            page = page
                .push(text(note).size(12).color(colour))
                .push(Space::new().height(10));
        }
        scroll(page.push(column![
            introduction,
            Space::new().height(28),
            metrics,
            Space::new().height(28),
            card_rule(),
            Space::new().height(24),
            calendar_card(&self.days),
        ]))
    }

    fn install_banner(&self) -> Option<Element<'_, Message>> {
        let (line, offer, tone) = self.install_problem()?.banner();

        // Filled only when the news is good. A saturated green button is the
        // brightest thing in an amber banner, and the eye lands on the way out
        // before it has read the problem - the same reason setup's error screen
        // outlines its button instead of filling it.
        let inviting = tone == ACCENT;

        Some(
            container(
                row![
                    text(line).size(12.5).color(tone),
                    Space::new().width(Fill),
                    action_msg(offer, inviting, Message::BeginSetup),
                ]
                .align_y(iced::Center),
            )
            .padding([10, 12])
            .width(Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(mix(BG, tone, 0.055))),
                border: Border {
                    radius: CARD_RADIUS.into(),
                    width: HAIRLINE,
                    color: mix(BG, tone, 0.22),
                },
                ..Default::default()
            })
            .into(),
        )
    }

    fn attention(&self) -> Vec<(Color, String)> {
        let mut notes = Vec::new();
        if let Some(problem) = &self.service_error {
            notes.push((ERR, problem.clone()));
        }
        // A save that failed used to be a line in a footer on the screen it
        // failed on. The footer is gone, and this is where the faults are - a
        // read-only config or a full disk is exactly the class of thing this
        // card exists to carry.
        if let Some(problem) = &self.save_error {
            notes.push((ERR, format!("Couldn't save: {problem}")));
        }
        if let Some(problem) = &self.daemon.problem {
            notes.push((ERR, problem.clone()));
        }
        if let update::Status::Available(tag) = &self.update {
            notes.push((ACCENT, format!("{tag} is available to install.")));
        }
        notes
    }
}
