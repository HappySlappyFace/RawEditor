use iced::widget::image::Handle;
use iced::widget::{
    container, column, mouse_area, row, scrollable, stack, text, Space,
};
use crate::ui::widgets::button;
use crate::ui::widgets::fade_image::fade_image;
use iced::{Alignment, Background, Border, Color, Element, Length};
use std::collections::HashMap;

use crate::app::message::Message;
use crate::app::state::RawEditor;
use crate::database::models::{Image as ImageData, MARKED_FOR_REMOVAL_RATING};
use crate::ui;
use crate::ui::icons::ICON_FONT;
use crate::ui::theme;

const THUMB_PRESET_SMALL: f32 = 150.0;
const THUMB_PRESET_MEDIUM: f32 = 190.0;
const THUMB_PRESET_LARGE: f32 = 230.0;
const THUMB_PRESET_XL: f32 = 280.0;
const LIBRARY_TOOLBAR_HEIGHT: f32 = 56.0;
const LIBRARY_STATUS_HEIGHT: f32 = 42.0;
const LIBRARY_SIDEBAR_WIDTH: f32 = 250.0;

/// Gap between cards, and the margin around the whole grid.
const GRID_SPACING: f32 = 12.0;
const GRID_PADDING: f32 = 12.0;
/// A card is its thumbnail plus 7px of padding on every side.
const CARD_CHROME: f32 = 14.0;
/// Rows built beyond each edge of the viewport, so a fast wheel flick lands
/// on already-built cards rather than a blank frame.
const GRID_OVERSCAN_ROWS: usize = 2;

/// Layout of the virtualised library grid.
///
/// The grid used to push EVERY image into one `Wrap`, so each view rebuild —
/// which iced does after every message — built, diffed and laid out tens of
/// thousands of widgets for a large catalogue, to show a few dozen. Only the
/// rows near the viewport are built now, with spacers standing in for the
/// rest so the scrollbar still spans the whole catalogue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridGeometry {
    pub columns: usize,
    pub card_width: f32,
    /// Card height plus the gap below it.
    pub row_height: f32,
    pub viewport_height: f32,
}

impl GridGeometry {
    pub fn new(thumbnail_size: f32, viewport: (f32, f32)) -> Self {
        let card_width = thumbnail_size + CARD_CHROME;
        let card_height = thumbnail_size * 0.75 + CARD_CHROME;
        let usable = (viewport.0 - 2.0 * GRID_PADDING).max(card_width);
        let columns = ((usable + GRID_SPACING) / (card_width + GRID_SPACING)).floor().max(1.0);
        Self {
            columns: columns as usize,
            card_width,
            row_height: card_height + GRID_SPACING,
            viewport_height: viewport.1.max(1.0),
        }
    }

    /// The editor's grid as it is (or is about to be) laid out.
    pub fn of(editor: &RawEditor) -> Self {
        let viewport = if editor.grid_viewport.0 > 0.0 {
            editor.grid_viewport
        } else {
            // First frame, before the scrollable has reported its size:
            // everything the window has that the chrome around it doesn't.
            let w = editor.window_size;
            (
                w.width - LIBRARY_SIDEBAR_WIDTH,
                w.height
                    - super::layout::TITLE_BAR_HEIGHT
                    - LIBRARY_TOOLBAR_HEIGHT
                    - LIBRARY_STATUS_HEIGHT,
            )
        };
        Self::new(editor.thumbnail_size, viewport)
    }

    pub fn total_rows(&self, images: usize) -> usize {
        images.div_ceil(self.columns)
    }

    /// Content-space y of a row's top edge.
    pub fn row_top(&self, row: usize) -> f32 {
        GRID_PADDING + row as f32 * self.row_height
    }

    /// Rows `first..last` to build for this scroll offset: those on screen
    /// plus `GRID_OVERSCAN_ROWS` either side, clamped to the catalogue.
    pub fn visible_rows(&self, scroll_y: f32, total_rows: usize) -> (usize, usize) {
        let y = (scroll_y - GRID_PADDING).max(0.0);
        let first = ((y / self.row_height).floor() as usize)
            .saturating_sub(GRID_OVERSCAN_ROWS)
            .min(total_rows);
        let last = (((y + self.viewport_height) / self.row_height).ceil() as usize
            + GRID_OVERSCAN_ROWS)
            .min(total_rows);
        (first, last.max(first))
    }

    /// Indices into the image list covered by rows `first..last`. Clamped:
    /// with a stale scroll offset (a filter just shrank the catalogue) the
    /// rows can start past the last, partially filled one.
    pub fn image_range(&self, first: usize, last: usize, images: usize) -> std::ops::Range<usize> {
        let end = (last * self.columns).min(images);
        (first * self.columns).min(end)..end
    }

    /// The scroll offset that brings `row` fully into view, or `None` if it
    /// already is.
    pub fn scroll_to_reveal(&self, row: usize, scroll_y: f32) -> Option<f32> {
        let top = self.row_top(row) - GRID_PADDING;
        let bottom = self.row_top(row) + self.row_height;
        if top < scroll_y {
            Some(top.max(0.0))
        } else if bottom > scroll_y + self.viewport_height {
            Some((bottom - self.viewport_height).max(0.0))
        } else {
            None
        }
    }
}

/// Scroll routing (`handlers::scroll`) targets the grid by this id.
pub fn grid_scroll_id() -> scrollable::Id {
    scrollable::Id::new("library_grid")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThumbnailState {
    Ready,
    Queued,
    Missing,
}

fn thumbnail_state(img: &ImageData) -> ThumbnailState {
    if img.file_status == "deleted" {
        ThumbnailState::Missing
    } else if img.cache_path_thumb.is_none() {
        ThumbnailState::Queued
    } else {
        ThumbnailState::Ready
    }
}

/// Delegates to the shared predicate so the grid and the filmstrip can never
/// disagree about what looks selected.
fn is_image_selected(editor: &RawEditor, image_id: i64) -> bool {
    editor.is_selected(image_id)
}

fn metric_pill<'a>(label: &'a str, value: impl ToString, color: Color) -> Element<'a, Message> {
    container(
        row![
            text(label).size(theme::size::CAPTION).style(|_| text::Style {
                color: Some(theme::colours().text_secondary)
            }),
            text(value.to_string())
                .size(theme::size::CAPTION)
                .style(move |_| text::Style { color: Some(color) }),
        ]
        .spacing(theme::space::S)
        .align_y(Alignment::Center),
    )
    .padding([4, 8])
    .style(|_| container::Style {
        background: Some(Background::Color(theme::colours().surface_raised)),
        border: Border {
            radius: theme::radius::DIALOG.into(),
            width: 1.0,
            color: theme::colours().border_subtle,
        },
        ..Default::default()
    })
    .into()
}

fn compact_path(path: &str) -> String {
    let path = std::path::Path::new(path);
    let parts: Vec<String> = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect();

    if parts.len() >= 2 {
        let last = &parts[parts.len() - 1];
        let before_last = &parts[parts.len() - 2];
        let compact = format!(".../{}/{}", before_last, last);
        if compact.len() <= 34 {
            return compact;
        }
    }

    let as_str = path.to_string_lossy();
    if as_str.len() <= 36 {
        return as_str.to_string();
    }
    let tail: String = as_str
        .chars()
        .rev()
        .take(33)
        .collect::<Vec<char>>()
        .into_iter()
        .rev()
        .collect();
    format!("...{}", tail)
}

fn collect_folder_stats(editor: &RawEditor) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for image in &editor.images {
        if let Some(parent) = std::path::Path::new(&image.path).parent() {
            let key = parent.to_string_lossy().to_string();
            *counts.entry(key).or_insert(0) += 1;
        }
    }

    let mut folders: Vec<(String, usize)> = counts.into_iter().collect();
    // Alphabetical, not by count: the list is scrollable now, so a stable
    // order the user can predict beats surfacing the biggest folders. It used
    // to sort by count and truncate to 16, which made smaller folders
    // invisible — and, now that folders can be removed from here, unremovable.
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    folders
}

fn is_thumb_preset_active(current: f32, preset: f32) -> bool {
    (current - preset).abs() < 1.0
}

fn view_thumbnail_size_presets<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    row![
        text("Card size")
            .size(theme::size::SMALL)
            .style(|_| text::Style { color: Some(theme::colours().text_secondary) }),
        button(text("S").size(theme::size::CAPTION))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_SMALL))
            .padding([4, 8])
            .style(ui::styles::chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_SMALL
            ))),
        button(text("M").size(theme::size::CAPTION))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_MEDIUM))
            .padding([4, 8])
            .style(ui::styles::chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_MEDIUM
            ))),
        button(text("L").size(theme::size::CAPTION))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_LARGE))
            .padding([4, 8])
            .style(ui::styles::chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_LARGE
            ))),
        button(text("XL").size(theme::size::CAPTION))
            .on_press(Message::SetThumbnailSize(THUMB_PRESET_XL))
            .padding([4, 8])
            .style(ui::styles::chip(is_thumb_preset_active(
                editor.thumbnail_size,
                THUMB_PRESET_XL
            ))),
    ]
    .spacing(theme::space::S)
    .align_y(Alignment::Center)
    .into()
}

/// `thumbnail` is `RawEditor::thumbnail_handle` — the edited proxy for an
/// edited image, the camera's embedded preview otherwise.
fn view_image_card<'a>(
    img: &'a ImageData,
    thumbnail: Option<Handle>,
    is_selected: bool,
    size: f32,
) -> Element<'a, Message> {
    let thumb_width = size;
    let thumb_height = size * 0.75;
    let thumb_state = thumbnail_state(img);
    let has_loaded_thumb = thumb_state == ThumbnailState::Ready;

    // The thumbnail fades up over the placeholder when its cache tier lands,
    // and crossfades when an edited proxy replaces it. The image is the TOP
    // layer at a fixed position whether or not it is ready yet: `FadeImage`
    // can only animate a change it was mounted to see. Keyed by image id, so
    // a virtualised slot that scrolls onto another photo just shows it.
    let placeholder: Element<'a, Message> = if has_loaded_thumb {
        Space::new(Length::Fill, Length::Fill).into()
    } else {
        let placeholder_label = match thumb_state {
            ThumbnailState::Missing => "Missing...",
            ThumbnailState::Queued => "Queued...",
            ThumbnailState::Ready => "Generating...",
        };
        container(
            column![
                text(ui::icons::TH_LARGE).font(ICON_FONT).size(theme::size::DISPLAY),
                text(placeholder_label).size(theme::size::CAPTION).style(|_| text::Style {
                    color: Some(theme::colours().text)
                })
            ]
            .spacing(theme::space::S)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    };
    let image_content: Element<'a, Message> = stack![
        placeholder,
        fade_image(
            img.id as u64,
            thumbnail.filter(|_| has_loaded_thumb),
            thumb_width,
            thumb_height,
        )
        .content_fit(iced::ContentFit::Cover),
    ]
    .into();

    let top_left_badge: Element<'a, Message> = if has_loaded_thumb && img.flag == 1 {
        container(
            row![
                text(ui::icons::CHECK).font(ICON_FONT).size(theme::size::CAPTION),
                text("Pick").size(theme::size::CAPTION)
            ]
            .spacing(theme::space::XS)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.15, 0.62, 0.25, 0.95))),
            border: Border {
                radius: theme::radius::DIALOG.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else if has_loaded_thumb && img.flag == -1 {
        container(
            row![
                text(ui::icons::TIMES).font(ICON_FONT).size(theme::size::CAPTION),
                text("Reject").size(theme::size::CAPTION)
            ]
            .spacing(theme::space::XS)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.85, 0.20, 0.20, 0.95))),
            border: Border {
                radius: theme::radius::DIALOG.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else {
        container(Space::with_width(0)).into()
    };

    let top_right_badge: Element<'a, Message> = if has_loaded_thumb
        && img.rating == MARKED_FOR_REMOVAL_RATING
    {
        container(
            row![
                text(ui::icons::TIMES).font(ICON_FONT).size(theme::size::CAPTION),
                text("Marked").size(theme::size::CAPTION)
            ]
            .spacing(theme::space::XS)
            .align_y(Alignment::Center),
        )
        .padding([3, 7])
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.85, 0.55, 0.05, 0.95))),
            border: Border {
                radius: theme::radius::DIALOG.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    } else if has_loaded_thumb && img.rating > 0 {
        let stars = vec![ui::icons::STAR; img.rating as usize].join(" ");
        container(text(stars).font(ICON_FONT).size(theme::size::CAPTION))
            .padding([3, 7])
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.65))),
                border: Border {
                    radius: theme::radius::DIALOG.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    } else {
        container(Space::with_width(0)).into()
    };

    let image_stack = stack![
        container(image_content)
            .width(Length::Fixed(thumb_width))
            .height(Length::Fixed(thumb_height))
            .clip(true)
            .style(|_| container::Style {
                // The placeholder tone; a Cover-fit thumbnail hides it.
                background: Some(Background::Color(theme::colours().surface_raised)),
                border: Border {
                    radius: 0.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        container(top_left_badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .padding(8),
        container(top_right_badge)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Top)
            .padding(8),
    ];

    // Selection and hover live on the button so they *transition* (see
    // `ui::widgets::button`): a click crossfades the highlight in, hovering
    // warms the frame. Keyed by id like the image.
    button(image_stack)
        .on_press(Message::ImageSelected(img.id))
        .width(Length::Fixed(thumb_width + 14.0))
        .height(Length::Fixed(thumb_height + 14.0))
        .padding(7)
        .clip(true)
        .key(img.id as u64)
        .style(move |_theme, status| card_style(is_selected, status))
        .into()
}

/// The contact-sheet frame around a thumbnail. Square corners throughout.
fn card_style(is_selected: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (fill, edge) = match (is_selected, hovered) {
        (true, false) => (theme::colours().accent_selection, theme::colours().accent_selection_border),
        (true, true) => (theme::colours().accent_selection, theme::colours().accent_text),
        (false, false) => (theme::colours().bg, theme::colours().border_subtle),
        (false, true) => (theme::colours().surface_raised, theme::colours().border_strong),
    };
    button::Style {
        background: Some(Background::Color(fill)),
        border: Border { radius: 0.0.into(), width: 1.0, color: edge },
        text_color: theme::colours().text_strong,
        ..Default::default()
    }
}

/// The virtualised thumbnail grid — see [`GridGeometry`].
fn view_grid<'a>(editor: &'a RawEditor, images: &[&'a ImageData]) -> Element<'a, Message> {
    let geom = GridGeometry::of(editor);
    let total_rows = geom.total_rows(images.len());
    let (first, last) = geom.visible_rows(editor.grid_scroll_y, total_rows);

    let mut rows = column![Space::with_height(Length::Fixed(first as f32 * geom.row_height))];
    for chunk in images[geom.image_range(first, last, images.len())].chunks(geom.columns) {
        let cards = chunk.iter().fold(row![].spacing(GRID_SPACING), |cards, img| {
            cards.push(view_image_card(
                img,
                editor.thumbnail_handle(img),
                is_image_selected(editor, img.id),
                editor.thumbnail_size,
            ))
        });
        rows = rows.push(container(cards).height(Length::Fixed(geom.row_height)));
    }
    rows = rows.push(Space::with_height(Length::Fixed(
        (total_rows - last) as f32 * geom.row_height,
    )));

    // Fixed to exactly `columns` cards so a short last row stays left-aligned
    // under the rest, as the old Wrap laid it out.
    let grid_width =
        geom.columns as f32 * geom.card_width + (geom.columns - 1) as f32 * GRID_SPACING;
    let content = container(rows.width(Length::Fixed(grid_width)))
        .width(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .padding(GRID_PADDING);

    mouse_area(
        scrollable(content)
            .id(grid_scroll_id())
            .on_scroll(|viewport| {
                let bounds = viewport.bounds();
                Message::GridScrolled(viewport.absolute_offset().y, bounds.width, bounds.height)
            })
            .height(Length::Fill)
            .width(Length::Fill),
    )
    .on_enter(Message::GridHovered(true))
    .on_exit(Message::GridHovered(false))
    .into()
}

fn view_filter_bar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let chip = |label: Element<'a, Message>, rating: u8| {
        button(label)
            .on_press(Message::SetMinRating(rating))
            .padding([4, 10])
            .style(ui::styles::chip(editor.min_filter_rating == rating))
    };
    let mut bar = row![chip(text("All").size(theme::size::SMALL).into(), 0)]
        .spacing(theme::space::S)
        .align_y(Alignment::Center);
    // One star and the threshold ("★ 3+") instead of a run of stars: the
    // chips stay one width and the number reads at a glance.
    for rating in 1..=5u8 {
        let label = if rating == 5 { "5".to_string() } else { format!("{rating}+") };
        bar = bar.push(chip(
            row![
                text(ui::icons::STAR).font(ICON_FONT).size(theme::size::CAPTION),
                text(label).size(theme::size::SMALL),
            ]
            .spacing(theme::space::XS)
            .align_y(Alignment::Center)
            .into(),
            rating,
        ));
    }
    bar.into()
}

fn import_button(icon: &'static str, label: &'static str, msg: Message) -> Element<'static, Message> {
    button(
        row![text(icon).font(ICON_FONT).size(theme::size::BODY), text(label).size(theme::size::BODY)]
            .spacing(theme::space::M)
            .align_y(Alignment::Center),
    )
    .on_press(msg)
    .padding([7, 10])
    .width(Length::Fill)
    .style(ui::styles::NeutralButton::style)
    .into()
}

fn view_library_sidebar<'a>(
    editor: &'a RawEditor,
    folder_stats: &[(String, usize)],
    total_count: usize,
) -> Element<'a, Message> {
    let selected_filter = editor.library_folder_filter.clone();
    let all_active = selected_filter.is_none();

    let mut folders_column = column![
        button(
            row![
                text("All Photos").size(theme::size::SMALL),
                Space::with_width(Length::Fill),
                text(total_count.to_string()).size(theme::size::CAPTION)
            ]
            .align_y(Alignment::Center)
        )
        .on_press(Message::SetLibraryFolderFilter(None))
        .padding([7, 9])
        .width(Length::Fill)
        .style(ui::styles::list_item(all_active)),
    ]
    .spacing(theme::space::S);

    for (folder, count) in folder_stats {
        let is_active = selected_filter
            .as_ref()
            .map(|active| active == folder)
            .unwrap_or(false);
        folders_column = folders_column.push(
            row![
                button(
                    row![
                        text(compact_path(folder)).size(theme::size::CAPTION),
                        Space::with_width(Length::Fill),
                        text(count.to_string()).size(theme::size::CAPTION)
                    ]
                    .align_y(Alignment::Center),
                )
                .on_press(Message::SetLibraryFolderFilter(Some(folder.clone())))
                .padding([6, 8])
                .width(Length::Fill)
                .style(ui::styles::list_item(is_active)),
                // Forget this folder — catalogue only, files untouched.
                button(text(ui::icons::TIMES).font(ICON_FONT).size(theme::size::CAPTION))
                    .on_press(Message::RemoveFolderRequested(folder.clone()))
                    .padding([6, 8])
                    .style(ui::styles::ghost_button),
            ]
            .spacing(theme::space::XXS)
            .align_y(Alignment::Center),
        );
    }

    container(
        column![
            ui::text::group_label("Import"),
            import_button(ui::icons::FOLDER_OPEN, "Import Folder", Message::ImportFolder),
            import_button(ui::icons::FOLDER, "Import from Camera", Message::ImportFromCamera),
            Space::with_height(Length::Fixed(10.0)),
            ui::text::group_label("Folders"),
            scrollable(folders_column).height(Length::Fill),
        ]
        .spacing(theme::space::S)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .padding(10)
    .width(Length::Fixed(LIBRARY_SIDEBAR_WIDTH))
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(theme::colours().bg)),
        border: Border {
            width: 1.0,
            color: theme::colours().border_subtle,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn view_library_toolbar<'a>(editor: &'a RawEditor) -> Element<'a, Message> {
    let export_btn = button(
        row![
            text(ui::icons::SAVE).font(ICON_FONT).size(theme::size::BODY),
            text("Export").size(theme::size::BODY),
        ]
        .spacing(theme::space::M)
        .align_y(Alignment::Center),
    )
    .on_press(Message::OpenExportModal)
    .padding([7, 14])
    .style(ui::styles::AccentButton::style);

    container(
        row![
            ui::text::title("Library"),
            view_filter_bar(editor),
            Space::with_width(Length::Fill),
            export_btn,
        ]
        .spacing(theme::space::L)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    // 8px vertically, not 12: the bar is LIBRARY_TOOLBAR_HEIGHT (56) tall,
    // and the Export button needs ~37px. At 12px it got 32, and iced lays out
    // text given less than one line of height at zero width — the button
    // rendered as an empty pill.
    .padding([8, 12])
    .style(|_| container::Style {
        background: Some(Background::Color(theme::colours().surface)),
        border: Border {
            width: 1.0,
            color: theme::colours().border_subtle,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

fn view_status_bar<'a>(
    editor: &'a RawEditor,
    filtered_count: usize,
    total_count: usize,
    selected_count: usize,
    cached_count: usize,
    deleted_count: usize,
) -> Element<'a, Message> {
    let folder_label = editor
        .library_folder_filter
        .as_ref()
        .map(|folder| compact_path(folder))
        .unwrap_or_else(|| "All folders".to_string());

    container(
        row![
            metric_pill(
                "Items",
                format!("{}/{}", filtered_count, total_count),
                theme::colours().text,
            ),
            metric_pill("Selected", selected_count, theme::colours().accent_text),
            metric_pill(
                "Cached",
                format!("{}/{}", cached_count, filtered_count),
                theme::colours().ok,
            ),
            metric_pill("Deleted", deleted_count, theme::colours().error),
            metric_pill("Folder", folder_label, theme::colours().text),
            iced::widget::horizontal_space(),
            view_thumbnail_size_presets(editor),
            Space::with_width(Length::Fixed(8.0)),
            text(&editor.status).size(theme::size::SMALL).style(|_| text::Style {
                color: Some(theme::colours().text_secondary)
            }),
        ]
        .spacing(theme::space::XL)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(LIBRARY_STATUS_HEIGHT))
    .padding([0, 10])
    .clip(true)
    .align_y(iced::alignment::Vertical::Center)
    .style(|_| container::Style {
        background: Some(Background::Color(theme::colours().bg)),
        border: Border {
            width: 1.0,
            color: theme::colours().border_subtle,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

pub fn view_library(editor: &RawEditor) -> Element<'_, Message> {
    let folder_stats = collect_folder_stats(editor);
    // The shared predicate, not a local copy of it — see
    // `RawEditor::displayed_images` for why the per-tab filters live in one
    // place (marked-for-removal images intentionally stay visible here).
    let filtered_images: Vec<&ImageData> = editor.displayed_images();

    let cached_count = filtered_images
        .iter()
        .filter(|img| img.cache_path_thumb.is_some())
        .count();
    let deleted_count = filtered_images
        .iter()
        .filter(|img| thumbnail_state(img) == ThumbnailState::Missing)
        .count();
    let total_count = editor.images.len();
    let filtered_count = filtered_images.len();
    let selected_count = filtered_images
        .iter()
        .filter(|img| is_image_selected(editor, img.id))
        .count();

    let main_panel: Element<'_, Message> = if filtered_images.is_empty() {
        container(
            container(
                column![
                    text("No images match this filter").size(theme::size::DISPLAY),
                    text("Try another folder/rating or import a new directory.")
                        .size(theme::size::LABEL)
                        .style(|_| text::Style {
                            color: Some(theme::colours().text_muted)
                        }),
                    button(
                        row![
                            text(ui::icons::FOLDER_OPEN).font(ICON_FONT),
                            text("Import Folder")
                        ]
                        .spacing(theme::space::M)
                    )
                    .on_press(Message::ImportFolder)
                    .style(ui::styles::NeutralButton::style)
                    .padding([10, 14]),
                ]
                .spacing(theme::space::XXL)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .width(Length::Fill)
            .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::colours().bg)),
            ..Default::default()
        })
        .into()
    } else {
        container(view_grid(editor, &filtered_images))
        .width(Length::Fill)
        .height(Length::Fill)
        .clip(true)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::colours().bg)),
            ..Default::default()
        })
        .into()
    };

    let body = row![
        view_library_sidebar(editor, &folder_stats, total_count),
        container(main_panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(true);

    let toolbar = container(view_library_toolbar(editor))
        .width(Length::Fill)
        .height(Length::Fixed(LIBRARY_TOOLBAR_HEIGHT))
        .clip(true)
        .align_y(iced::alignment::Vertical::Top);

    let status = container(view_status_bar(
        editor,
        filtered_count,
        total_count,
        selected_count,
        cached_count,
        deleted_count,
    ))
    .width(Length::Fill)
    .height(Length::Fixed(LIBRARY_STATUS_HEIGHT))
    .clip(true)
    .align_y(iced::alignment::Vertical::Bottom);

    // Toolbar and status bar are LATER stack children than `body`, not earlier
    // column siblings — scrolled Image thumbnails in the grid don't respect
    // ancestor clip bounds and would otherwise bleed over them. The Spaces
    // reserve their bands in the layout flow so `body` is sized identically.
    container(stack![
        column![
            Space::with_height(Length::Fixed(LIBRARY_TOOLBAR_HEIGHT)),
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .clip(true),
            Space::with_height(Length::Fixed(LIBRARY_STATUS_HEIGHT)),
        ]
        .width(Length::Fill)
        .height(Length::Fill),
        container(toolbar)
            .width(Length::Fill)
            .align_y(iced::alignment::Vertical::Top),
        container(status)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Bottom),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .clip(true)
    .style(|_| container::Style {
        background: Some(Background::Color(theme::colours().bg)),
        ..Default::default()
    })
    .into()
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    #[test]
    fn columns_fill_the_viewport_without_overflowing() {
        // card 234px + 12px gaps: 4 cards need 4*234 + 3*12 = 972 (+24 padding).
        assert_eq!(GridGeometry::new(220.0, (996.0, 800.0)).columns, 4);
        assert_eq!(GridGeometry::new(220.0, (995.0, 800.0)).columns, 3);
        // Never zero, however narrow.
        assert_eq!(GridGeometry::new(220.0, (10.0, 800.0)).columns, 1);
    }

    #[test]
    fn visible_rows_cover_the_viewport_plus_overscan() {
        let g = GridGeometry::new(220.0, (996.0, 400.0)); // row_height 191
        assert_eq!(g.visible_rows(0.0, 100), (0, 3 + GRID_OVERSCAN_ROWS));
        // Scrolled 10 rows down: starts overscan rows above the first visible.
        let (first, last) = g.visible_rows(g.row_top(10), 100);
        assert_eq!(first, 10 - GRID_OVERSCAN_ROWS);
        assert!(last >= 10 + 3);
        // Clamped at the end of the catalogue.
        assert_eq!(g.visible_rows(1e9, 100), (100, 100));
    }

    /// 10 images in rows of 4: the third row holds only two, and a stale
    /// offset past the end must yield an empty range, not a panic.
    #[test]
    fn image_range_never_runs_past_the_catalogue() {
        let g = GridGeometry::new(220.0, (996.0, 400.0));
        assert_eq!(g.columns, 4);
        assert_eq!(g.image_range(0, 3, 10), 0..10);
        assert_eq!(g.image_range(2, 3, 10), 8..10);
        let (first, last) = g.visible_rows(1e9, g.total_rows(10));
        assert!(g.image_range(first, last, 10).is_empty());
    }

    #[test]
    fn reveal_scrolls_only_when_the_row_is_off_screen() {
        let g = GridGeometry::new(220.0, (996.0, 400.0));
        assert_eq!(g.scroll_to_reveal(0, 0.0), None);
        // Row 5 is below a 400px viewport at offset 0: bring its bottom in.
        let y = g.scroll_to_reveal(5, 0.0).expect("row 5 is off screen");
        assert!((g.row_top(5) + g.row_height - (y + 400.0)).abs() < 1e-3);
        // Row 0 is above when scrolled down: bring its top back.
        assert_eq!(g.scroll_to_reveal(0, 2000.0), Some(0.0));
    }
}
