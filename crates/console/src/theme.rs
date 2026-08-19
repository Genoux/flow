//! The design tokens the window is drawn from, and the three functions that
//! move between them.
//!
//! Taken from the island in overlay.rs so the window and the overlay read as
//! one product: its ground colour, its restraint, one warm accent that only
//! ever means "live".
//!
//! One module rather than a block at the top of `main.rs`, because these are
//! the values every other module reaches for: a colour that lives beside the
//! screen that happens to use it first is a colour the next screen invents
//! again slightly differently.

use iced::Color;

pub(crate) const BG: Color = Color { r: 0.039, g: 0.043, b: 0.055, a: 1.0 }; // #0A0B0E
pub(crate) const FG: Color = Color { r: 0.925, g: 0.929, b: 0.937, a: 1.0 }; // #ECEDEF
/// Secondary text: labels, captions, the second line of a tile. Lifted from
/// #7C828C, which sat at 4.4:1 on a card and so failed the same contrast bar
/// the body text clears comfortably. Quiet is a job for weight and size here,
/// not for a grey that has to be squinted at.
pub(crate) const MUTED: Color = Color { r: 0.541, g: 0.565, b: 0.604, a: 1.0 }; // #8A909A
/// The quietest text in the product - 11px meta: timestamps, month names, the
/// label half of a label/value pair. Also lifted, from #4E545C at 2.2:1, which
/// is decoration rather than text at that size.
pub(crate) const FAINT: Color = Color { r: 0.424, g: 0.451, b: 0.490, a: 1.0 }; // #6C737D
pub(crate) const LINE: Color = Color { r: 0.106, g: 0.118, b: 0.137, a: 1.0 }; // #1B1E23
/// The lifted surface a card sits on. Half a step off the ground rather than a
/// full one: a card already carries a hairline and a shadow, and three depth
/// cues on one rectangle - repeated down a page of them - is what turned the
/// Overview into a stack of grey plates. The container recedes; the words on
/// it are the thing to see.
pub(crate) const RAISED: Color = Color { r: 0.082, g: 0.090, b: 0.106, a: 1.0 }; // #15171B
/// What a rail item sits on when it is the current section. `RAISED` was doing
/// this job too, and at card weight against the same ground the selected item
/// was nearly invisible - a rail should stay quiet, but quiet still has to be
/// legible.
pub(crate) const RAIL_ON: Color = Color { r: 0.145, g: 0.157, b: 0.176, a: 1.0 }; // #25282D
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
pub(crate) const EDGE: Color = Color { r: 0.149, g: 0.165, b: 0.184, a: 1.0 }; // #262A2F
pub(crate) const ACCENT: Color = Color { r: 0.180, g: 0.835, b: 0.451, a: 1.0 }; // #2ED573
pub(crate) const ERR: Color = Color { r: 0.831, g: 0.451, b: 0.420, a: 1.0 }; // #D4736B
/// "Nothing to do" - and the same green as ACCENT, deliberately. It used to be
/// a muted olive of its own, to sit at ERR's weight in a row of dots, and next
/// to the accent it just read as a second, dirtier green. One green in the
/// product, one meaning per colour: green is fine, red is not.
pub(crate) const OK: Color = ACCENT;
pub(crate) const ON_ACCENT: Color = Color { r: 0.078, g: 0.082, b: 0.059, a: 1.0 };

pub(crate) const RAIL_WIDTH: f32 = 176.0;

/// How far back the Overview keeps daily counts. Not the number of weeks on
/// screen: the calendar draws as many weeks as the pane is wide, so this is
/// the ceiling for a very wide window - two years, past which a dictation
/// habit is better summarised than drawn.
pub(crate) const CALENDAR_WEEKS: usize = 104;
pub(crate) const CALENDAR_DAYS: usize = CALENDAR_WEEKS * 7;

/// One calendar cell, and the gap between two of them. Fixed rather than
/// stretched to the pane: a heat cell is read by colour, and colour on a
/// square is easier to compare than colour on a rectangle that changes shape
/// with the window. The pane's width buys more weeks instead.
pub(crate) const CELL: f32 = 12.0;
pub(crate) const CELL_GAP: f32 = 3.0;
/// The Mon/Wed/Fri gutter down the left of the grid.
pub(crate) const WEEKDAY_GUTTER: f32 = 28.0;

/// The one gap between everything on the Overview - between cards, and
/// between the tiles in a row. A page of cards with three different gaps in it
/// reads as a page of cards that were placed one at a time.
pub(crate) const GAP: f32 = 12.0;

/// How much room a scrolling list keeps at each end, inside the viewport so
/// that it scrolls with the content rather than being clipped away with it.
pub(crate) const SCROLL_PAD: f32 = 18.0;
/// Where the first thing on a page sits. Padding inside the scroll, so it
/// is room above the heading and below the last row - it moves with them,
/// and at either end of the page there is still air.
pub(crate) const PAGE_TOP: f32 = 32.0;

/// The pane's left margin, and the base for the content's right margin. One
/// constant so the page reads as evenly framed even though the two sides get
/// there differently - the left is a pane pad, the right is the same room
/// plus the scrollbar's own footprint.
pub(crate) const PANE_INSET: f32 = 32.0;
/// Where the text stops on the right: the left margin plus clearance for the
/// scrollbar (width 4, margin 2 on each side of its track), so a visible
/// scroller never sits on top of a letter.
pub(crate) const CONTENT_RIGHT: f32 = PANE_INSET;
/// History keeps its text on the page grid while the hover surface reaches
/// past it. The same value is the row's inner inset and the surface's bleed.
pub(crate) const ENTRY_INSET: f32 = 12.0;
/// Top and bottom padding for one row in a settings list, on both sides of
/// every hairline between them. Anything else that borders a hairline - the
/// footer's, for one - uses this too, so a divider always has the same air
/// whichever two things it happens to be sitting between.
pub(crate) const ROW_PAD: f32 = 16.0;
/// The docked footer's band, above and below its content. Equal on both
/// sides, because a bar reads as a bar only when its content sits in the
/// middle of it - the old pairing borrowed `ROW_PAD` above and the page's
/// whole bottom margin below, which left the button hung near the hairline
/// with a stretch of dead floor under it.
pub(crate) const FOOT_PAD: f32 = 18.0;

/// How long each motion takes. Only two things move - a toggle's knob and a
/// rail item warming under the pointer - because those are the two that
/// acknowledge something the user just did. Long enough that the easing
/// curve is visible rather than read as a snap.
pub(crate) const KNOB: u64 = 220;
pub(crate) const FADE: u64 = 200;
/// How long a copied row keeps its paper lit before the icon goes back to
/// waiting for a hover. Long enough to be read, short enough that it is never
/// still saying it by the time you look again.
pub(crate) const COPIED: u64 = 1600;

/// Quartic ease-out. Everything here moves fastest at the start and settles
/// gently into place rather than stopping - a steeper tail than cubic, which
/// is what turns "arrives" into "settles".
pub(crate) fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

/// 0.0 at `since`, 1.0 once `ms` has passed.
pub(crate) fn progress(since: std::time::Instant, now: std::time::Instant, ms: u64) -> f32 {
    ease_out(now.saturating_duration_since(since).as_millis() as f32 / ms as f32)
}

/// Blend two colours, for hover states and the settling of a fade.
pub(crate) fn mix(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color::from_rgba(
        from.r + (to.r - from.r) * t,
        from.g + (to.g - from.g) * t,
        from.b + (to.b - from.b) * t,
        from.a + (to.a - from.a) * t,
    )
}
