//! The Overview's activity calendar, and the numbers that go under it.
//!
//! One cell per day coloured by how many words were dictated - the GitHub
//! contribution graph shape, because it is the shape people already know how
//! to read at a glance.

use crate::card::card;
use crate::format::{commas, plural};
use crate::theme::{
    mix, ACCENT, BG, CALENDAR_WEEKS, CELL, CELL_GAP, FAINT, FG, LINE, MUTED, RAISED,
    WEEKDAY_GUTTER,
};
use crate::{history, Message};
use iced::widget::{column, container, responsive, row, text, tooltip, Space};
use iced::{Background, Border, Color, Element, Fill, Length};

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
pub(crate) fn current_streak(days: &[history::Day]) -> usize {
    let ending_today = days.iter().rev().take_while(|day| day.words > 0).count();
    if ending_today > 0 || days.len() < 2 {
        return ending_today;
    }
    days[..days.len() - 1].iter().rev().take_while(|day| day.words > 0).count()
}

/// The longest run of consecutive active days anywhere in the buffer.
pub(crate) fn longest_streak(days: &[history::Day]) -> usize {
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
pub(crate) fn calendar_card(days: &[history::Day]) -> Element<'_, Message> {
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
        // Saturating because the grid reaches two years back and this is
        // day-since-epoch arithmetic: a clock set before 1972 makes the
        // subtraction negative and panics the window on open, which is a
        // worse answer to a wrong clock than drawing the grid from day zero.
        let first_day = (today + last_weekday as u64 + 1).saturating_sub(columns as u64 * 7);

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
        "Nothing yet. Hold the chord and say something.".to_string()
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

#[cfg(test)]
mod tests {
    use super::{current_streak, heat_ceiling};

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
}
