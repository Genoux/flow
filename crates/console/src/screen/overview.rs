//! The first screen: what Flow is doing, and what it still needs.

use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Fill, Font};

impl Console {
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
    pub(super) fn overview_section(&self) -> Element<'_, Message> {
        // Three states, and the first one rules out the other two:
        //
        //   Not running · needs setup   (no action - the banner owns the way out)
        //   Running                     [Stop]
        //   Not running                 [Start]
        //
        // An unfinished setup is reported as its own state rather than as the
        // daemon's, because the daemon's answer cannot be trusted there: a
        // process left over from before the models went missing still answers
        // the socket, and "Flow is running" over a half-installed product is
        // the one sentence this page must never say.
        let status = status_of(self.incomplete(), self.daemon.activity);
        let needs_setup = status == Status::NeedsSetup;
        let running = status == Status::Running;
        let installed = self.models.iter().filter(|m| m.installed).count();
        // Red, not grey. Grey reads as a resting state, and this is not one:
        // the product cannot dictate until it is dealt with. It is the same
        // severity as the missing-model notes below it, and the two saying it
        // in different colours would make one of them look optional.
        let (label, dot) = if needs_setup {
            ("Not running, needs setup", ERR)
        } else {
            activity_label(self.daemon.activity)
        };
        let verb = if running { "stop" } else { "start" };
        // Always in the row, even while systemd is mid-verb. Pulling it out
        // used to let the Fill space eat its width, so the status walked right
        // on every start and stop. Disabled is how a second press is refused.
        //
        // Gone entirely while setup is unfinished, though, rather than greyed:
        // a disabled Start still says that starting is the answer and that
        // something is stopping you. Nothing is - the models are not there, and
        // the only move is Finish setup in the banner below. Two controls
        // offering the way forward is one control too many.
        let mut header = row![
            text("Overview").size(22).color(FG),
            Space::new().width(Fill),
            pip(dot),
            Space::new().width(9),
            text(label)
                .size(13)
                .color(dot)
                .wrapping(text::Wrapping::None),
        ]
        .align_y(iced::Center);

        if !needs_setup {
            header = header.push(Space::new().width(16));
            header = header.push(action_msg(
                service_action_label(running),
                !running,
                self.service_pending
                    .is_none()
                    .then_some(Message::Service(verb)),
            ));
        }

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
                commas(crate::history::words(this_week)),
                trend(
                    crate::history::words(this_week),
                    crate::history::words(last_week)
                ),
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
                crate::history::duration(spoken),
                if dictations == 0 {
                    ("nothing this week".to_string(), MUTED)
                } else {
                    (
                        format!(
                            "{} average",
                            crate::history::duration(spoken / dictations as f32)
                        ),
                        MUTED,
                    )
                },
            ),
            Space::new().width(GAP),
            stat_tile(
                "Current streak",
                plural(streak as u32, "day"),
                (
                    format!(
                        "longest {}",
                        plural(longest_streak(&self.days) as u32, "day")
                    ),
                    MUTED,
                ),
            ),
        ];

        // Sized to fit the default window, so the common case does not scroll
        // at all. Scrolled rather than compressed when it does not fit: a user
        // who drags the window down to the minimum height should have to reach
        // the last card rather than have every card shrink to meet them. The
        // heading is in the scroll, same as every other page: a window this
        // short has to give the cards the room, not keep a title parked over
        // them.
        let mut page = column![];
        // Above everything, because an unfinished install is not a fact about
        // this page - it is a fact about the product, and the way out of it is
        // the only thing on screen worth pressing.
        if let Some(banner) = self.finish_setup_banner() {
            page = page.push(banner);
            page = page.push(Space::new().height(GAP));
        }

        scroll(page.push(column![
            header,
            Space::new().height(SCROLL_PAD),
            self.setup_card(installed),
            Space::new().height(GAP),
            kpis,
            Space::new().height(GAP),
            calendar_card(&self.days),
        ]))
    }

    /// The way out of a stopped setup.
    ///
    /// Only reachable by having pressed Stop, so it is worded as the resumption
    /// of something started rather than as an offer of something extra - and it
    /// does not say what the download costs, because that was on screen when
    /// the decision was made.
    fn finish_setup_banner(&self) -> Option<Element<'_, Message>> {
        if !self.incomplete() {
            return None;
        }

        Some(
            container(
                row![
                    text("Setup isn't finished, so Flow isn't running yet.")
                        .size(12.5)
                        .color(ACCENT),
                    Space::new().width(Fill),
                    action_msg("Finish setup", true, Message::BeginSetup),
                ]
                .align_y(iced::Center),
            )
            .padding([10, 12])
            .width(Fill)
            .style(|_| container::Style {
                background: Some(Background::Color(mix(BG, ACCENT, 0.055))),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: mix(BG, ACCENT, 0.22),
                },
                ..Default::default()
            })
            .into(),
        )
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
                // The verb leads. "super shift d to hold" reads as a chord
                // named after an action; "hold super shift d" is the
                // instruction it was meant to be.
                format!(
                    "{} {}",
                    if self.settings.push_to_talk {
                        "hold"
                    } else {
                        "tap"
                    },
                    self.settings.hotkey.replace('+', " "),
                ),
            ),
            Space::new().width(44),
            fact(
                "Microphone",
                clip(self.input.as_deref().unwrap_or("system default"), 38,),
            ),
            Space::new().width(Fill),
            fact("Models", format!("{installed} of {}", self.models.len())),
        ];

        let mut body = column![];
        let notes = self.attention();
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
        let when = latest
            .map(|entry| crate::history::ago(entry.at, crate::history::now()))
            .unwrap_or_default();

        let heading = row![
            text("Last dictation").size(11).color(FAINT),
            Space::new().width(Fill),
            // `ago` already says "just now" for the last minute; empty means
            // the timestamp is missing or in the future, and no label is
            // better than a confident wrong one.
            text(when).size(11).font(Font::MONOSPACE).color(FAINT),
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
            None => text("nothing yet - hold the chord and say something")
                .size(12.5)
                .color(FAINT),
        };

        column![heading, Space::new().height(5), line].into()
    }

    /// Everything on this page that is worth doing something about, in the
    /// order it would bite. Empty when there is nothing to do, and the status
    /// card then collapses to one line - a dashboard that always has a row of
    /// warnings in it teaches people not to read the warnings.
    fn attention(&self) -> Vec<(Color, String)> {
        let mut notes = Vec::new();
        if let Some(problem) = &self.service_error {
            notes.push((ERR, problem.clone()));
        }
        if let Some(problem) = &self.daemon.problem {
            notes.push((ERR, problem.clone()));
        }
        // A missing model is not listed here. The banner above this card
        // already says setup is unfinished and carries the button that fixes
        // it, and the Models fact below counts what arrived - a third line
        // saying the same thing only splits one answer into three.
        if let update::Status::Available(tag) = &self.update {
            notes.push((ACCENT, format!("{tag} is available to install.")));
        }
        notes
    }
}
