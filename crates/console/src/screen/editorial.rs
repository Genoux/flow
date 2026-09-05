use crate::theme::{BANNER_CORNER, BG};
use crate::Message;
use iced::widget::{canvas, column, container, image, stack, text, text_input, Space};
use iced::{Background, Border, Color, Element, Fill, Font};

#[derive(Clone, Copy)]
pub(super) enum Photo {
    Woodland,
    Portrait,
}

pub(super) fn banner(
    title: &'static str,
    description: &'static str,
    wide: bool,
    compact: bool,
    photo: Photo,
) -> Element<'static, Message> {
    let height = if compact {
        180.0
    } else if wide {
        250.0
    } else {
        280.0
    };
    let photograph = image(banner_image(photo));
    let photograph = match photo {
        Photo::Woodland => photograph,
        Photo::Portrait => photograph.crop(iced::Rectangle {
            x: 0,
            y: 0,
            width: 1600,
            height: 700,
        }),
    };
    let photograph = photograph
        .width(Fill)
        .height(height)
        .content_fit(iced::ContentFit::Cover);
    let shade = container(Space::new().width(Fill).height(Fill))
        .width(Fill)
        .height(Fill)
        .style(move |_| container::Style {
            background: Some(Background::Gradient(
                iced::gradient::Linear::new(iced::Radians(0.0))
                    .add_stop(0.0, Color::from_rgba(0.02, 0.02, 0.02, 0.6))
                    .add_stop(
                        1.0,
                        Color::from_rgba(0.02, 0.02, 0.02, if wide { 0.08 } else { 0.32 }),
                    )
                    .into(),
            )),
            ..Default::default()
        });
    let introduction = container(
        column![
            text(title)
                .font(Font::with_name("Noto Serif Display"))
                .size(if compact {
                    28
                } else if wide {
                    36
                } else {
                    32
                })
                .line_height(1.12)
                .color(Color::from_rgb8(245, 242, 232)),
            Space::new().height(14),
            text(description)
                .size(13)
                .line_height(1.55)
                .color(Color::from_rgb8(208, 206, 194)),
        ]
        .max_width(420),
    )
    .padding(if wide { 30 } else { 24 })
    .width(Fill)
    .height(height)
    .align_y(iced::Center);
    let corners = canvas::Canvas::new(BannerCorners)
        .width(Fill)
        .height(height);
    stack![photograph, shade, corners, introduction].into()
}

pub(super) fn input_style(
    _: &iced::Theme,
    _: text_input::Status,
    amount: f32,
) -> text_input::Style {
    use crate::theme::*;
    text_input::Style {
        background: Background::Color(mix(BG, RAISED, 0.65)),
        border: Border {
            radius: RADIUS.into(),
            width: HAIRLINE,
            color: mix(EDGE, FG, amount * 0.3),
        },
        icon: MUTED,
        placeholder: MUTED,
        value: FG,
        selection: mix(BG, ACCENT, 0.35),
    }
}

fn banner_image(photo: Photo) -> image::Handle {
    static WOODLAND: std::sync::LazyLock<image::Handle> = std::sync::LazyLock::new(|| {
        image::Handle::from_bytes(
            include_bytes!("../../../../assets/style-woodland.jpg").as_slice(),
        )
    });
    static PORTRAIT: std::sync::LazyLock<image::Handle> = std::sync::LazyLock::new(|| {
        image::Handle::from_bytes(
            include_bytes!("../../../../assets/vocabulary-nick-fancher.jpg").as_slice(),
        )
    });
    match photo {
        Photo::Woodland => WOODLAND.clone(),
        Photo::Portrait => PORTRAIT.clone(),
    }
}

struct BannerCorners;

impl canvas::Program<Message> for BannerCorners {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &iced::Theme,
        bounds: iced::Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let radius = BANNER_CORNER
            .min(bounds.width / 2.0)
            .min(bounds.height / 2.0);
        for (x, y, dx, dy) in [
            (0.0, 0.0, 1.0, 1.0),
            (bounds.width, 0.0, -1.0, 1.0),
            (bounds.width, bounds.height, -1.0, -1.0),
            (0.0, bounds.height, 1.0, -1.0),
        ] {
            let point = |u, v| iced::Point::new(x + dx * u, y + dy * v);
            let path = canvas::Path::new(|builder| {
                builder.move_to(point(-1.0, -1.0));
                builder.line_to(point(radius, -1.0));
                builder.line_to(point(radius, 0.0));
                for step in 0..=64 {
                    let angle = step as f32 / 64.0 * std::f32::consts::FRAC_PI_2;
                    let u = radius * (1.0 - angle.sin().max(0.0).sqrt());
                    let v = radius * (1.0 - angle.cos().max(0.0).sqrt());
                    builder.line_to(point(u, v));
                }
                builder.line_to(point(-1.0, radius));
                builder.close();
            });
            // Matching the page ground masks both photo and shade with one continuous curve.
            frame.fill(&path, BG);
        }
        vec![frame.into_geometry()]
    }
}
