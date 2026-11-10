//! The Offhand logo as the console draws it. The files are the brand's own
//! exports in `branding/offhand/`, with the lettering already converted to
//! outlines, so the console needs no copy of the typeface to show the name.

use std::sync::LazyLock;

use iced::widget::svg::{self, Handle};
use iced::widget::Svg;
use iced::{Color, Element, Length};

static LOCKUP: LazyLock<Handle> = LazyLock::new(|| {
    Handle::from_memory(
        include_bytes!("../../../branding/offhand/offhand-logo-bone.svg").as_slice(),
    )
});

/// Width over height of the lockup file's own canvas.
const LOCKUP_ASPECT: f32 = 4582.35 / 900.0;

/// The mark and the name together, `height` tall and drawn in `color`.
pub(crate) fn lockup<'a, Message: 'a>(height: f32, color: Color) -> Element<'a, Message> {
    Svg::new(LOCKUP.clone())
        .width(Length::Fixed(height * LOCKUP_ASPECT))
        .height(Length::Fixed(height))
        .style(move |_theme, _status| svg::Style { color: Some(color) })
        .into()
}
