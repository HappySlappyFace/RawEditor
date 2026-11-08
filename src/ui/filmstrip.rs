use iced::widget::{container, mouse_area, row, scrollable, text, stack, Space};
use crate::ui::widgets::button;
use crate::ui::widgets::fade_image::fade_image;
use iced::{Background, Border, Color, Element, Length, Theme};

use crate::database::models::{Image, MARKED_FOR_REMOVAL_RATING};
use crate::app::message::Message;
use crate::ui::icons;  // Phase 58: Icon constants

/// Id shared by the Cull and Develop filmstrips — only one is ever mounted at
/// a time, so a single static id is enough to target it with `scroll_by`.
pub fn scroll_id() -> scrollable::Id {
    scrollable::Id::new("filmstrip")
}

/// Thumbnail size, and the horizontal padding either side of each one.
const THUMB_W: f32 = 140.0;
use super::theme;
const THUMB_H: f32 = 105.0;
const ITEM_PAD_X: f32 = 16.0;
/// Width of one filmstrip slot. Fixed, which is what lets the strip be
/// virtualised and lets navigation compute where an image sits.
pub const ITEM_WIDTH: f32 = THUMB_W + 2.0 * ITEM_PAD_X;
/// Slots built beyond each edge of the visible strip.
const OVERSCAN_ITEMS: usize = 4;

/// Slots `first..last` worth building for a scroll offset and visible width.
pub fn visible_items(scroll_x: f32, view_width: f32, count: usize) -> (usize, usize) {
    let first = ((scroll_x.max(0.0) / ITEM_WIDTH).floor() as usize)
        .saturating_sub(OVERSCAN_ITEMS)
        .min(count);
    let last = (((scroll_x.max(0.0) + view_width) / ITEM_WIDTH).ceil() as usize + OVERSCAN_ITEMS)
        .min(count);
    (first, last.max(first))
}

/// Where [`scroll_to_reveal`] puts a slot it has to scroll to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reveal {
    /// Just far enough: the slot lands at the edge it was beyond, as the
    /// first or last one visible, and one slot leaves the other end. For
    /// stepping through the strip, where centring would throw the user's
    /// place half a screen away on every edge crossing.
    Edge,
    /// Centred, for arriving somewhere new (a tab switch) with context on
    /// both sides.
    Center,
}

/// The scroll offset that brings slot `index` into view, or `None` if it is
/// already fully visible.
pub fn scroll_to_reveal(index: usize, scroll_x: f32, view_width: f32, how: Reveal) -> Option<f32> {
    let left = index as f32 * ITEM_WIDTH;
    let right = left + ITEM_WIDTH;
    if left >= scroll_x && right <= scroll_x + view_width {
        return None;
    }
    let offset = match how {
        Reveal::Center => left - (view_width - ITEM_WIDTH) / 2.0,
        Reveal::Edge if left < scroll_x => left,
        Reveal::Edge => right - view_width,
    };
    Some(offset.max(0.0))
}

/// Render the filmstrip timeline at the bottom of the Develop and Cull tabs.
///
/// Takes the whole editor rather than a selection set so it can use
/// `RawEditor::is_selected` — the same predicate the library grid uses. It
/// previously read `multi_selection` alone, which disagreed with the grid
/// whenever `selected_image_id` was not also in the set.
///
/// Virtualised like the library grid: only the slots near the visible window
/// are built. This view is rebuilt on every frame of a momentum glide, and it
/// used to rebuild every thumbnail in the catalogue each time.
pub fn view<'a>(
    images: &[&'a Image],
    editor: &crate::app::state::RawEditor,
) -> Element<'a, Message> {
    let view_width = if editor.filmstrip_view_w > 0.0 {
        editor.filmstrip_view_w
    } else {
        editor.window_size.width
    };
    let (first, last) = visible_items(editor.filmstrip_scroll_x, view_width, images.len());

    let mut thumbnails: Vec<Element<'a, Message>> = Vec::with_capacity(last - first + 2);
    thumbnails.push(Space::with_width(Length::Fixed(first as f32 * ITEM_WIDTH)).into());

    for img in &images[first..last] {
        let img = *img;
        let is_selected = editor.is_selected(img.id);

        // The edited proxy when the image has edits, so the strip shows the
        // photos as edited. Images still waiting on their cache tiers get a
        // blank slot rather than being skipped: every slot must be ITEM_WIDTH
        // wide for the index → position arithmetic above to hold. A
        // `FadeImage` either way, so the thumbnail fades in when it lands.
        let handle = editor.thumbnail_handle(img);
        // Only an empty slot gets the placeholder tone: a portrait thumbnail
        // is letterboxed, and its side bars should show the selection tint.
        let waiting = handle.is_none();
        let img_widget: Element<'a, Message> =
            container(fade_image(img.id as u64, handle, THUMB_W, THUMB_H))
                .style(move |_| container::Style {
                    background: waiting
                        .then_some(Background::Color(theme::colours().surface_raised)),
                    ..Default::default()
                })
                .into();

        // Phase 56: Overlay star rating (or the "marked for removal" badge,
        // which reuses this same field via a reserved sentinel — must be
        // checked first or `img.rating as usize` below would try to build a
        // 255-element star vec) on the bottom-left corner.
        let rating_overlay = if img.rating == MARKED_FOR_REMOVAL_RATING {
            Element::from(container(
                container(
                    row![
                        text(icons::TIMES).size(theme::size::SMALL).font(crate::app::view::ICON_FONT),
                        text("Marked").size(theme::size::SMALL),
                    ]
                    .spacing(theme::space::XS)
                    .align_y(iced::Alignment::Center)
                )
                .padding(iced::Padding { top: 2.0, right: 4.0, bottom: 2.0, left: 4.0 })
                .style(|_theme: &Theme| {
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(0.85, 0.55, 0.05, 0.9))),
                        border: Border { radius: theme::radius::CONTROL.into(), ..Default::default() },
                        ..Default::default()
                    }
                })
            )
            .padding(iced::Padding { top: 0.0, right: 0.0, bottom: 2.0, left: 2.0 })
            .align_y(iced::alignment::Vertical::Bottom)
            .align_x(iced::alignment::Horizontal::Left)
            .width(Length::Fill)
            .height(Length::Fill))
        } else if img.rating > 0 {
            let stars_text = vec![icons::STAR; img.rating as usize].join(" ");
            Element::from(container(
                container(
                    text(stars_text)  // Space between stars
                        .size(theme::size::LABEL)
                        .color(Color::from_rgb(1.0, 0.8, 0.2))  // Gold
                        .font(crate::app::view::ICON_FONT)  // Phase 57: Use embedded font
                )
                .padding(iced::Padding {
                    top: 2.0,
                    right: 4.0,
                    bottom: 2.0,
                    left: 4.0,
                })
                .style(|_theme: &Theme| {
                    container::Style {
                        background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.6))),  // Dark semi-transparent
                        border: Border {
                            radius: theme::radius::CONTROL.into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    }
                })
            )
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 2.0,  // 2px from bottom
                left: 2.0,    // 2px from left
            })
            .align_y(iced::alignment::Vertical::Bottom)
            .align_x(iced::alignment::Horizontal::Left)
            .width(Length::Fill)
            .height(Length::Fill))
        } else {
            Element::from(container(text("")).width(Length::Fill).height(Length::Fill))
        };
        
        // Stack image with rating overlay
        let thumbnail_content = stack![
            img_widget,
            rating_overlay
        ];
        
        // Wrap in container for selection styling
        let thumbnail_container = container(thumbnail_content)
            .padding(iced::Padding {
                top: 4.0,
                right: ITEM_PAD_X,
                bottom: 4.0,
                left: ITEM_PAD_X,
            })
            .style(move |_theme: &Theme| {
                // Selection is drawn by the button below, so it can fade.
                let mut style = container::Style::default();
                // Phase 83: Flag Feedback
                if img.flag == -1 {
                    style.text_color = Some(Color::from_rgba(1.0, 1.0, 1.0, 0.5)); // Dim text if any
                }
                style
            });
            
        // Phase 83: Apply opacity/overlay
        let mut final_content = Element::from(thumbnail_container);
        
        if img.flag == -1 {
            // Rejected: Overlay a semi-transparent black box with "X"
            let reject_overlay = container(
                text(icons::TIMES).font(crate::app::view::ICON_FONT).size(theme::size::DISPLAY).color(Color::from_rgba(1.0, 0.2, 0.2, 0.8))
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(|_| container::Style { background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.5))), ..Default::default() });
            
            final_content = Element::from(stack![
                final_content,
                reject_overlay
            ]);
        } else if img.flag == 1 {
             // Picked: Overlay a small green flag/check
            let pick_overlay = container(
                text(icons::CHECK).font(crate::app::view::ICON_FONT).size(theme::size::HEADING).color(Color::from_rgba(0.2, 1.0, 0.2, 0.9))
            )
            .padding(4)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .width(Length::Fill)
            .height(Length::Fill);
            
            final_content = Element::from(stack![
                final_content,
                pick_overlay
            ]);
        }

        // Wrap in button for clicking. Selection and hover are its style, so
        // moving through the strip crossfades the highlight from one frame to
        // the next; keyed by id because the strip is virtualised.
        let thumbnail_button = button(final_content)
            .padding(0)  // Remove button's default padding
            .on_press(Message::ImageSelected(img.id))
            .key(img.id as u64)
            .style(move |_theme: &Theme, status| {
                let tint = match (is_selected, status) {
                    (true, _) => Some(theme::colours().control_active),
                    (false, button::Status::Hovered | button::Status::Pressed) => {
                        Some(theme::colours().control)
                    }
                    (false, _) => None,
                };
                button::Style {
                    background: tint.map(Background::Color),
                    border: Border::default(),
                    text_color: theme::colours().text_strong,
                    ..Default::default()
                }
            })
            .into(); // Convert to Element
        
        thumbnails.push(thumbnail_button);
    }
    thumbnails.push(
        Space::with_width(Length::Fixed((images.len() - last) as f32 * ITEM_WIDTH)).into(),
    );

    // Create row from thumbnails - minimal spacing
    let film_row = row(thumbnails)
        .spacing(0)
        .padding(iced::Padding {
            top: 2.0,
            right: 0.0,
            bottom: 10.0,  // Extra space for scrollbar
            left: 0.0,
        });
    // Make it scrollable horizontally
    let scrollable_film = scrollable(film_row)
        .id(scroll_id())
        .on_scroll(|viewport| {
            Message::FilmstripScrolled(viewport.absolute_offset().x, viewport.bounds().width)
        })
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new()
                .width(8)
                .scroller_width(8)
        ))
        .style(|_theme: &Theme, _status| {
            scrollable::Style {
                container: container::Style {
                    background: Some(Background::Color(theme::colours().bg)),
                    ..Default::default()
                },
                vertical_rail: scrollable::Rail {
                    background: None,
                    border: Border::default(),
                    scroller: scrollable::Scroller {
                        color: theme::colours().border_strong,
                        border: Border::default(),
                    },
                },
                horizontal_rail: scrollable::Rail {
                    background: Some(Background::Color(theme::colours().surface)), // Track color
                    border: Border::default(),
                    scroller: scrollable::Scroller {
                        color: theme::colours().border_control, // Draggable scroller color
                        border: Border::default(),
                    },
                },
                gap: None,
            }
        });
    
    // Dark background container (no padding to avoid wasted space)
    let scroll_surface = container(scrollable_film)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(0)
        .clip(true)
        .style(|_theme: &Theme| {
            container::Style {
                background: Some(Background::Color(theme::colours().bg)),
                ..Default::default()
            }
        });

    // Wheel interceptor, stacked ABOVE the scrollable (later stack children
    // get first crack at events in iced). Capturing the wheel event here
    // short-circuits iced's Stack dispatch before the scrollable underneath
    // ever sees it, so its hardcoded ~60px/line native scroll never fires —
    // handlers::scroll::handle_filmstrip_wheel drives 100% of the motion,
    // giving a single continuous physics model with no jump/seam.
    let wheel_capture = mouse_area(Space::new(Length::Fill, Length::Fill))
        .on_scroll(Message::FilmstripWheel);

    stack![scroll_surface, wheel_capture]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_items_cover_the_strip_plus_overscan() {
        // 10 slots fit in 1720px.
        assert_eq!(visible_items(0.0, 1720.0, 1000), (0, 10 + OVERSCAN_ITEMS));
        let (first, last) = visible_items(100.0 * ITEM_WIDTH, 1720.0, 1000);
        assert_eq!(first, 100 - OVERSCAN_ITEMS);
        assert_eq!(last, 110 + OVERSCAN_ITEMS);
        assert_eq!(visible_items(0.0, 1720.0, 3), (0, 3));
    }

    #[test]
    fn reveal_centres_an_off_screen_slot_and_leaves_visible_ones() {
        assert_eq!(scroll_to_reveal(2, 0.0, 1720.0, Reveal::Center), None);
        let x = scroll_to_reveal(50, 0.0, 1720.0, Reveal::Center).expect("slot 50 is off screen");
        let centre = 50.0 * ITEM_WIDTH + ITEM_WIDTH / 2.0;
        assert!((x + 1720.0 / 2.0 - centre).abs() < 1e-3);
        // Never scrolls before the start.
        assert_eq!(scroll_to_reveal(0, 500.0, 1720.0, Reveal::Center), Some(0.0));
    }

    /// Stepping off either end brings the slot to that edge — the last one
    /// visible going right, the first going left — not to the centre.
    #[test]
    fn edge_reveal_moves_just_far_enough() {
        let view = 1720.0;
        assert_eq!(scroll_to_reveal(5, 0.0, view, Reveal::Edge), None);
        // The view holds exactly ten slots: slot 10 is the first off it.
        let x = scroll_to_reveal(10, 0.0, view, Reveal::Edge).expect("slot 10 is off screen");
        assert!((x + view - 11.0 * ITEM_WIDTH).abs() < 1e-3, "right edges should meet");
        assert!((x - ITEM_WIDTH).abs() < 1e-3, "one step, not half a screen: {x}");
        // Going left: the slot becomes the first visible.
        let x = scroll_to_reveal(20, 22.0 * ITEM_WIDTH, view, Reveal::Edge).unwrap();
        assert_eq!(x, 20.0 * ITEM_WIDTH);
    }
}
