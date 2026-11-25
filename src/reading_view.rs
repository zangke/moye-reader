use std::{ops::Range, rc::Rc, sync::LazyLock};

use regex::Regex;

use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::{
    book::{Block, BlockKind, BookInfo, Link, NoteKind, Passage, PointInText},
    library::{Paper, Tone},
    pagination::{LineStart, Pagination, Row},
    reader::{Drag, ImageState, NotePopup, Reader},
    style::{self, Palette, READING_FONT},
};

const PAGE_MAX_WIDTH: Pixels = px(780.);
// const SPREAD_PAGE_MAX_WIDTH: Pixels = px(686.);
// const WIDE_SPREAD_PAGE_MAX_WIDTH: Pixels = px(854.);
const PAGE_TOP_INSET: Pixels = px(6.);
const PAGE_BOTTOM_INSET: Pixels = px(12.);
const PAGE_GAP: Pixels = px(12.);

struct NoteMarker {
    range: Range<usize>,
    color: Hsla,
}

impl Reader {
    /// 页内容
    pub fn page(&self, p: Palette, window: &Window, cx: &Context<Self>) -> AnyElement {
        let view = cx.entity();
        if let Some(layout) = self.pagination.as_ref().filter(|_| self.is_paged()) {
            let first = self.visible_pages().start;
            let leaves = div()
                .h(layout.area.height + PAGE_TOP_INSET + PAGE_BOTTOM_INSET)
                .overflow_hidden()
                .flex()
                .child(self.page_leaf(first, layout, p, &view, window))
                .when(self.is_spread(), |spread| {
                    spread
                        .gap(PAGE_GAP)
                        .child(self.page_leaf(first + 1, layout, p, &view, window))
                });
            return div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .child(leaves)
                .into_any_element();
        }
        list(self.list.clone(), move |index, window, app| {
            view.read(app).page_row(index, p, &view, window, None).0
        })
        .size_full()
        .flex_1()
        .min_h_0()
        .into_any_element()
    }

    fn page_leaf(
        &self,
        index: usize,
        layout: &Pagination,
        p: Palette,
        view: &Entity<Self>,
        window: &Window,
    ) -> impl IntoElement {
        let area = layout.area;
        let inset = self.page_inset(Some(area));
        let marker_left = inset - if self.is_spread() { px(15.) } else { px(25.) };
        div()
            .id(("leaf", index))
            .w(area.width)
            .h_full()
            .pt(PAGE_TOP_INSET)
            .pb(PAGE_BOTTOM_INSET)
            .flex_shrink_0()
            .children(
                layout
                    .pages
                    .get(index)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(fragment, part)| {
                        let marked = self.bookmarks().iter().any(|mark| {
                            mark.section == self.section
                                && ((mark.item == 0 && index == 0 && fragment == 0)
                                    || (mark.item == part.start.item
                                        && mark.byte >= part.start.byte
                                        && part.end_byte.is_none_or(|end| mark.byte < end)))
                        });
                        div()
                            .relative()
                            .w_full()
                            .h(part.height)
                            .overflow_hidden()
                            .child(
                                div().absolute().top(-part.offset).w_full().child(
                                    self.page_row(part.start.item, p, view, window, Some(area))
                                        .0,
                                ),
                            )
                            .when(marked, |el| {
                                el.child(div().absolute().left(marker_left).top(px(8.)).child(
                                    style::icon(
                                        style::BOOKMARKED,
                                        if self.is_spread() { 12. } else { 14. },
                                        p.green,
                                    ),
                                ))
                            })
                    }),
            )
    }

    fn page_inset(&self, area: Option<Size<Pixels>>) -> Pixels {
        match area.filter(|_| self.is_spread()) {
            Some(area) => (area.width * 0.059).clamp(px(16.), px(42.)),
            None if self.handheld => px(28.),
            None => px(36.),
        }
    }

    pub fn reading_text_inset(&self, width: Pixels) -> Pixels {
        let area = self
            .pagination
            .as_ref()
            .filter(|_| self.is_paged())
            .map(|layout| layout.area);
        let leaf_width = area.map_or(width.min(PAGE_MAX_WIDTH), |area| area.width);
        let sheet_width = if area.is_some() && self.is_spread() {
            leaf_width * 2. + PAGE_GAP
        } else {
            leaf_width
        };
        ((width - sheet_width) / 2.).max(px(0.)) + self.page_inset(area)
    }

    pub fn sync_pagination(
        &mut self,
        pane: Size<Pixels>,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_paged() {
            if let Some(anchor) = self.pending_scroll_anchor.take() {
                let (mut row, text) = self.page_row(anchor.item, p, &cx.entity(), window, None);
                let metric = measure_row(&mut row, text, pane.width, window, cx);
                self.list.scroll_to(ListOffset {
                    item_ix: anchor.item,
                    offset_in_item: metric.offset_for(anchor.byte),
                });
                self.capture_position();
                self.schedule_save(window, cx);
            }
            return;
        }
        let page_width = if self.is_spread() {
            // let max_width = if !self.handheld && !self.rail_visible() {
            //     WIDE_SPREAD_PAGE_MAX_WIDTH
            // } else {
            //     SPREAD_PAGE_MAX_WIDTH
            // };
            // ((pane.width - px(16.) - PAGE_GAP) / 2.).min(max_width)
            (pane.width - px(16.) - PAGE_GAP) / 2.
        } else {
            // pane.width.min(PAGE_MAX_WIDTH)
            pane.width
        };
        tracing::info!("page_width {}", page_width);
        let area = size(
            page_width,
            (pane.height - PAGE_TOP_INSET - PAGE_BOTTOM_INSET).max(px(120.)),
        );
        let font_size = self.library.settings.size;
        let scroll_offset = self.pending_page_offset.take();
        if let Some(offset) = scroll_offset {
            let (mut row, text) =
                self.page_row(self.page_anchor.item, p, &cx.entity(), window, None);
            let metric = measure_row(&mut row, text, pane.width, window, cx);
            self.page_anchor.byte = metric.byte_for(offset);
        }
        let changed = self
            .pagination
            .as_ref()
            .is_none_or(|layout| layout.area != area || layout.font_size != font_size);
        if changed {
            let view = cx.entity();
            let rows = (0..=self.data().blocks.len())
                .map(|item| {
                    if item > 0
                        && matches!(self.data().blocks[item - 1].kind, BlockKind::Image { .. })
                    {
                        return Row {
                            height: area.height,
                            lines: Vec::new(),
                        };
                    }
                    let (mut row, text) = self.page_row(item, p, &view, window, Some(area));
                    measure_row(&mut row, text, area.width, window, cx)
                })
                .collect();
            self.pagination = Some(Pagination::new(area, font_size, rows));
        }
        if changed || self.pending_page_end || scroll_offset.is_some() {
            let layout = self.pagination.as_ref().unwrap();
            self.page_index = if std::mem::take(&mut self.pending_page_end) {
                let last = layout.pages.len() - 1;
                let first =
                    crate::pagination::page_range(last, layout.pages.len(), self.pages_per_turn())
                        .start;
                if let Some(fragment) = layout.pages[first].first() {
                    self.page_anchor = fragment.start;
                }
                first
            } else {
                layout.page_for(self.page_anchor)
            };
            self.capture_position();
            self.schedule_save(window, cx);
        }
    }

    /// 每页行内容
    fn page_row(
        &self,
        index: usize,
        p: Palette,
        view: &Entity<Self>,
        window: &Window,
        page_area: Option<Size<Pixels>>,
    ) -> (AnyElement, Option<(TextLayout, Pixels)>) {
        let frame = || div().w_full().flex().flex_col().items_center();
        let marked = page_area.is_none()
            && self
                .bookmarks()
                .iter()
                .any(|mark| mark.section == self.section && mark.item == index);
        let column = || {
            div()
                .w_full()
                // .max_w(page_area.map_or(PAGE_MAX_WIDTH, |area| area.width)) //TODO
                .px(self.page_inset(page_area))
                .relative()
                .when(marked, |row| {
                    row.child(
                        div()
                            .absolute()
                            .left(px(17.))
                            .top(px(if index == 0 { 50. } else { 8. }))
                            .child(style::icon(style::BOOKMARKED, 14., p.green)),
                    )
                })
        };
        if index == 0 {
            // Preserve opening headings and artwork without a synthetic title page.
            if self.data().title.is_empty()
                || self.data().blocks.first().is_some_and(|block| {
                    matches!(block.kind, BlockKind::Heading(_) | BlockKind::Image { .. })
                })
            {
                return (div().h_0().into_any_element(), None);
            }
            let compact = (page_area.is_some() && self.is_spread()) || self.handheld;
            return (
                // 章节标题
                frame()
                    .child(
                        column().pt(px(14.)).pb(px(16.)).child(
                            style::label(
                                self.data().title.clone(),
                                if compact { 27. } else { 32. },
                                p.ink,
                            )
                            .font_family(READING_FONT)
                            .font_weight(FontWeight::BOLD)
                            .line_height(px(if compact { 38. } else { 44. }))
                            .when(page_area.is_some(), |el| el.line_clamp(4)),
                        ),
                    )
                    .into_any_element(),
                None,
            );
        }
        let block_index = index - 1;
        let Some(block) = self.data().blocks.get(block_index) else {
            let next = self
                .neighbor(true)
                .map(|ix| (ix, self.book.title_at(ix, 0)));
            let view = view.downgrade();
            return (
                //继续阅读
                frame()
                    .child(column().pt(px(14.)).pb(px(42.)).child(
                        if let Some((section, name)) = next {
                            div()
                                .id("end-next")
                                .w_full()
                                .px(px(14.))
                                .py(px(12.))
                                .rounded(px(8.))
                                .border_1()
                                .border_color(rgb(p.line))
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(p.hover)))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(style::label("继续阅读", 12., p.dim))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(style::label(name, 13., p.green))
                                        .child(style::icon(style::RIGHT, 14., p.green)),
                                )
                                .on_click(move |_, _, app| {
                                    let _ = view
                                        .update(app, |reader, cx| reader.navigate(section, 0, cx));
                                })
                                .into_any_element()
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(12.))
                                .child(div().w(px(25.)).h(px(1.)).bg(rgb(p.line)))
                                .child(style::label("全书完", 12., p.dim))
                                .child(div().w(px(25.)).h(px(1.)).bg(rgb(p.line)))
                                .into_any_element()
                        },
                    ))
                    .into_any_element(),
                None,
            );
        };
        if let BlockKind::Image { path, alt } = &block.kind {
            let child = self.picture_view(path, alt, p, view, window, page_area);
            return (
                frame()
                    .child(
                        column()
                            .when_some(page_area, |el, area| {
                                el.h(area.height).flex().flex_col().justify_center()
                            })
                            .when(page_area.is_none(), |el| el.pb(px(18.)))
                            .child(child),
                    )
                    .into_any_element(),
                None,
            );
        }
        if block.kind == BlockKind::Rule {
            return (
                frame()
                    .child(column().py(px(21.)).child(style::line(p.line)))
                    .into_any_element(),
                None,
            );
        }
        let (paragraph, layout) = self.paragraph(block, block_index, p, view);
        (
            frame().child(column().child(paragraph)).into_any_element(),
            Some(layout),
        )
    }

    fn picture_view(
        &self,
        path: &str,
        alt: &str,
        p: Palette,
        view: &Entity<Self>,
        window: &Window,
        page_area: Option<Size<Pixels>>,
    ) -> AnyElement {
        if self.picture(&self.book.info, path).is_none() {
            schedule_picture(
                view.downgrade(),
                self.book.info.clone(),
                path.to_owned(),
                window,
            );
        }
        match self.picture(&self.book.info, path) {
            Some(ImageState::Ready(picture)) => {
                let width = picture.width as f32;
                let height = picture.height as f32;
                let (max_width, max_height) = page_area.map_or((530., 540.), |area| {
                    let caption = if alt.is_empty() { 0. } else { 46. };
                    (
                        f32::from(area.width - self.page_inset(page_area) * 2.).max(1.),
                        f32::from(area.height).min(850.) - caption,
                    )
                });
                let (max_width, max_height) = if self.book.info.format == crate::book::Format::Pdf {
                    page_area.map_or((760., 1080.), |_| (max_width, max_height))
                } else {
                    (max_width, max_height)
                };
                let factor = (max_height / height).min(max_width / width).min(1.);
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        img(picture.art.clone())
                            .w(px(width * factor))
                            .max_w_full()
                            .h(px(height * factor))
                            .object_fit(ObjectFit::Contain),
                    )
                    .when(!alt.is_empty(), |el| {
                        el.child(
                            style::label(alt.to_owned(), 12., p.dim)
                                .when(page_area.is_some(), |el| {
                                    el.line_height(px(18.)).line_clamp(2)
                                }),
                        )
                    })
                    .into_any_element()
            }
            Some(ImageState::Failed) => style::label(
                if self.book.info.format == crate::book::Format::Pdf {
                    "无法显示此 PDF 页面".to_owned()
                } else if alt.is_empty() {
                    "无法显示此插图".to_owned()
                } else {
                    format!("插图 · {alt}")
                },
                12.,
                p.dim,
            )
            .into_any_element(),
            _ => style::label(
                if self.book.info.format == crate::book::Format::Pdf {
                    "正在载入页面…"
                } else {
                    "正在载入插图…"
                },
                12.,
                p.dim,
            )
            .py(px(25.))
            .into_any_element(),
        }
    }

    fn paragraph(
        &self,
        block: &Block,
        block_index: usize,
        p: Palette,
        view: &Entity<Self>,
    ) -> (AnyElement, (TextLayout, Pixels)) {
        let font_size = f32::from(self.library.settings.size);
        let selected = self
            .selection
            .as_ref()
            .and_then(|passage| passage.in_block(self.section, block_index, block.text.len()));
        let mut underlines = vec![];
        for note in self.annotations().iter().rev() {
            if self.unlocated_annotations.contains(&note.id) {
                continue;
            }
            if let Some(range) = note
                .passage
                .in_block(self.section, block_index, block.text.len())
            {
                underlines.push((range, note.tone, self.revealed == Some(note.id)));
            }
        }
        let font_overrides = reading_font_overrides(block);
        let (highlights, note_markers) =
            self.highlights(block, selected.as_ref(), &underlines, &font_overrides, p);
        let text = StyledText::new(block.text.clone())
            .with_highlights(highlights)
            .with_font_family_overrides(font_overrides);
        let layout = text.layout().clone();
        let glyph_size = match block.kind {
            BlockKind::Heading(level) => font_size + if level <= 2 { 7. } else { 3. },
            BlockKind::Code => font_size * 0.84,
            _ => font_size,
        };
        let glyphs = self.glyph_layer(block, layout.clone(), px(glyph_size), view);
        let markers = (!note_markers.is_empty()).then(|| {
            let layout = layout.clone();
            let text = block.text.clone();
            canvas(
                |_, _, _| (),
                move |_, (), window, cx| {
                    paint_note_markers(&text, &layout, &note_markers, window, cx);
                },
            )
            .absolute()
            .inset_0()
        });
        let measured_layout = layout.clone();
        let motion_layout = layout.clone();
        let links = Rc::new(
            block
                .spans
                .iter()
                .filter_map(|span| {
                    span.style
                        .link
                        .clone()
                        .map(|link| (span.range.clone(), link))
                })
                .collect::<Vec<_>>(),
        );
        let watch = view.downgrade();
        let track = view.downgrade();
        let text_len = block.text.len();
        let actual_text = block.text.clone();
        let motion_text = actual_text.clone();
        let b = div()
            .id(("block", block_index))
            .relative()
            .w_full()
            .text_color(rgb(p.text))
            .font_family(READING_FONT)
            .text_size(px(font_size))
            .line_height(px(font_size * 1.58))
            .cursor(CursorStyle::IBeam)
            .pb(px(14.))
            .child(text)
            .when_some(glyphs, |body, overlay| body.child(overlay))
            .when_some(markers, |body, overlay| body.child(overlay))
            .on_mouse_down(MouseButton::Left, move |event, window, app| {
                let hit = layout.index_for_position(event.position);
                let byte = nearest_boundary(&actual_text, hit.unwrap_or_else(|ix| ix));
                let link = hit.ok().and_then(|ix| {
                    links
                        .iter()
                        .find(|(range, _)| range.contains(&ix))
                        .map(|(_, link)| link.clone())
                });
                let _ = watch.update(app, |reader, cx| {
                    if reader.editor.is_some() || reader.busy {
                        return;
                    }
                    reader.focus.focus(window, cx);
                    reader.settings_open = false;
                    reader.anchor = None;
                    reader.selection = None;
                    let point = PointInText {
                        block: block_index,
                        byte,
                    };
                    if event.click_count > 1 && (link.is_none() || event.click_count > 2) {
                        let range = if event.click_count > 2 {
                            0..text_len
                        } else {
                            word_at(&actual_text, byte)
                        };
                        reader.selection = Passage::new(
                            reader.section,
                            PointInText {
                                block: block_index,
                                byte: range.start,
                            },
                            PointInText {
                                block: block_index,
                                byte: range.end,
                            },
                        );
                        reader.anchor = Some(event.position);
                        reader.drag = None;
                    } else {
                        let annotation = hit.ok().and_then(|ix| {
                            reader.annotations().iter().rev().find_map(|note| {
                                (!note.body.trim().is_empty()
                                    && !reader.unlocated_annotations.contains(&note.id)
                                    && note
                                        .passage
                                        .in_block(reader.section, block_index, text_len)
                                        .is_some_and(|range| range.contains(&ix)))
                                .then_some(note.id)
                            })
                        });
                        reader.drag = Some(Drag {
                            start: point,
                            end: point,
                            link,
                            annotation,
                            origin: event.position,
                            moved: false,
                        });
                    }
                    cx.notify();
                });
            })
            .on_mouse_move(move |event, _, app| {
                if event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let byte = nearest_boundary(
                    &motion_text,
                    motion_layout
                        .index_for_position(event.position)
                        .unwrap_or_else(|ix| ix),
                );
                let _ = track.update(app, |reader, cx| {
                    let Some(drag) = reader.drag.as_mut() else {
                        return;
                    };
                    let point = PointInText {
                        block: block_index,
                        byte,
                    };
                    if drag.end == point {
                        return;
                    }
                    drag.end = point;
                    drag.moved = true;
                    reader.selection = Passage::new(reader.section, drag.start, point);
                    cx.notify();
                });
            });
        let same_quote = |index: Option<usize>| {
            block.quote_group.is_some()
                && index
                    .and_then(|index| self.data().blocks.get(index))
                    .is_some_and(|other| other.quote_group == block.quote_group)
        };
        let quote_top = if same_quote(block_index.checked_sub(1)) {
            0.
        } else {
            5.
        };
        let quote_continues = same_quote(Some(block_index + 1));
        let stylized = match block.kind {
            BlockKind::Heading(level) => b
                .pt(px(8.))
                .text_size(px(font_size + if level <= 2 { 7. } else { 3. }))
                .line_height(px(font_size * 2.1))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.ink)),
            BlockKind::Quote => b
                .pl(px(16.))
                .pr(px(12.))
                .pt(px(quote_top))
                .pb(px(if quote_continues { 0. } else { 9. }))
                .mb(px(if quote_continues { 0. } else { 17. }))
                .border_l_2()
                .border_color(rgb(p.green))
                .bg(rgb(p.green_bg))
                .text_color(rgb(p.green)),
            BlockKind::List => b.pl(px(22.)),
            BlockKind::Verse => b.pl(px(22.)).italic(),
            BlockKind::Code => b
                .font_family("Menlo")
                .text_size(px(font_size * 0.84))
                .p(px(11.))
                .mb(px(17.))
                .rounded(px(5.))
                .bg(rgb(p.rail)),
            _ => b,
        };
        let text_top = match block.kind {
            BlockKind::Heading(_) => 8.,
            BlockKind::Quote => quote_top,
            BlockKind::Code => 11.,
            _ => 0.,
        };
        (stylized.into_any_element(), (measured_layout, px(text_top)))
    }

    fn glyph_layer(
        &self,
        block: &Block,
        layout: TextLayout,
        em: Pixels,
        view: &Entity<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let glyphs = block
            .spans
            .iter()
            .filter_map(|span| Some((span.range.clone(), span.style.glyph.as_ref()?.clone())))
            .collect::<Vec<_>>();
        if glyphs.is_empty() {
            return None;
        }
        let view = view.downgrade();
        let book = self.book.info.clone();
        let frame = usize::from(self.library.settings.paper == Paper::Night);
        Some(
            canvas(
                move |_, window, cx| {
                    let Some(view) = view.upgrade() else {
                        return Vec::new();
                    };
                    let reader = view.read(cx);
                    let clip = window.content_mask().bounds;
                    let line_height = layout.line_height();
                    glyphs
                        .into_iter()
                        .filter_map(|(range, path)| {
                            let left = layout.position_for_index(range.start)?;
                            let right = layout.position_for_index(range.end)?;
                            // The end caret stays on the glyph's line even when the start is a wrap boundary.
                            let advance = if left.y == right.y {
                                (right.x - left.x).max(px(1.))
                            } else {
                                em
                            };
                            let slot = Bounds::new(
                                point(right.x - advance, right.y),
                                size(advance, line_height),
                            );
                            let visible = clip.intersect(&slot).size;
                            if visible.width <= px(0.) || visible.height <= px(0.) {
                                return None;
                            }
                            match reader.picture(&book, &path) {
                                Some(ImageState::Ready(picture)) => {
                                    let ratio = picture.width as f32 / picture.height as f32;
                                    let height = (advance / ratio).min(em);
                                    let width = height * ratio;
                                    let origin = slot.origin
                                        + point(
                                            (advance - width) / 2.,
                                            (line_height - height) / 2.,
                                        );
                                    Some((
                                        Bounds::new(origin, size(width, height)),
                                        picture.glyph(),
                                    ))
                                }
                                None => {
                                    schedule_picture(view.downgrade(), book.clone(), path, window);
                                    None
                                }
                                _ => None,
                            }
                        })
                        .collect::<Vec<_>>()
                },
                move |_, glyphs, window, _| {
                    for (bounds, image) in glyphs {
                        let _ = window.paint_image(
                            bounds,
                            bounds,
                            Corners::default(),
                            image,
                            frame,
                            false,
                        );
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
    }

    fn highlights(
        &self,
        block: &Block,
        selected: Option<&Range<usize>>,
        underlines: &[(Range<usize>, Tone, bool)],
        font_overrides: &[(Range<usize>, SharedString)],
        p: Palette,
    ) -> (Vec<(Range<usize>, HighlightStyle)>, Vec<NoteMarker>) {
        let comments = bracketed_notes(&block.text);
        let mut bounds = vec![];
        for range in block
            .spans
            .iter()
            .map(|span| &span.range)
            .chain(underlines.iter().map(|(range, _, _)| range))
            .chain(comments.iter().map(|(range, _)| range))
            .chain(font_overrides.iter().map(|(range, _)| range))
            .chain(selected)
        {
            if block.text.is_char_boundary(range.start) && block.text.is_char_boundary(range.end) {
                bounds.extend([range.start, range.end]);
            }
        }
        bounds.sort_unstable();
        bounds.dedup();
        let mut highlights = Vec::new();
        let mut markers: Vec<NoteMarker> = Vec::new();
        let mut spans = block.spans.iter().peekable();
        for indices in bounds.windows(2) {
            let range = indices[0]..indices[1];
            let mut style = HighlightStyle::default();
            while spans
                .peek()
                .is_some_and(|span| span.range.end <= range.start)
            {
                spans.next();
            }
            if let Some((_, kind)) = comments
                .iter()
                .find(|(comment, _)| comment.contains(&range.start))
            {
                style.color = Some(rgb(p.note_color(*kind)).into());
            }
            if let Some(span) = spans
                .peek()
                .filter(|span| span.range.contains(&range.start))
            {
                let inline = &span.style;
                let source_note = match &inline.link {
                    Some(Link::Local(target)) => self.book.notes.get(target),
                    _ => None,
                };
                let is_note = inline.note_ref || source_note.is_some();
                if inline.bold || is_note {
                    style.font_weight = Some(FontWeight::SEMIBOLD);
                }
                if inline.italic {
                    style.font_style = Some(FontStyle::Italic);
                }
                if let Some(kind) = inline.note_kind {
                    style.color = Some(rgb(p.note_color(kind)).into());
                }
                if let Some(color) = inline.color {
                    let color = p.epub_ink(color);
                    if color.s >= 0.13 || inline.note_kind.is_none() {
                        style.color = Some(color);
                    }
                }
                if is_note {
                    let accent: Hsla =
                        rgb(p.note_color(source_note.map_or(NoteKind::Footnote, |note| note.kind)))
                            .into();
                    style.color = Some(accent);
                    if range.start == span.range.start
                        && matches!(&inline.link, Some(Link::Local(target)) if !target.fragment.starts_with('\0'))
                        && inline.glyph.is_none()
                        && block
                            .text
                            .get(span.range.clone())
                            .is_some_and(is_note_marker_label)
                    {
                        markers.push(NoteMarker {
                            range: span.range.clone(),
                            color: accent,
                        });
                    }
                    if markers
                        .last()
                        .is_some_and(|marker| marker.range == span.range)
                    {
                        style.fade_out = Some(1.);
                    } else if source_note.is_some() {
                        style.background_color = Some(accent.opacity(0.11));
                    }
                } else if inline.link.is_some() {
                    style.color = Some(rgb(p.green).into());
                }
                if inline.marked {
                    style.background_color = Some(rgb(p.tone(Tone::Amber).1).into());
                }
                if (inline.link.is_some() && !is_note) || inline.underline {
                    style.underline = Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(rgb(p.green).into()),
                        wavy: false,
                    });
                }
                if inline.strike {
                    style.strikethrough = Some(StrikethroughStyle {
                        thickness: px(1.),
                        color: None,
                    });
                }
                if inline.glyph.as_ref().is_some_and(|path| {
                    matches!(
                        self.picture(&self.book.info, path),
                        Some(ImageState::Ready(_))
                    )
                }) {
                    style.fade_out = Some(1.);
                }
            }
            if let Some((_, tone, revealed)) = underlines
                .iter()
                .find(|(line, _, _)| line.contains(&range.start))
            {
                let (color, shade) = p.tone(*tone);
                style.underline = Some(UnderlineStyle {
                    thickness: if *revealed { px(2.) } else { px(1.4) },
                    color: Some(rgb(color).into()),
                    wavy: false,
                });
                style.background_color = Some(rgb(shade).into());
            }
            if selected.is_some_and(|selection| selection.contains(&range.start)) {
                style.background_color = Some(rgb(p.selection).into());
            }
            let next_font = font_overrides.partition_point(|(font, _)| font.end <= range.start);
            if style != HighlightStyle::default()
                || font_overrides
                    .get(next_font)
                    .is_some_and(|(font, _)| font.contains(&range.start))
            {
                highlights.push((range, style));
            }
        }
        (highlights, markers)
    }

    pub fn end_drag(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        let difference = event.position - drag.origin;
        if !drag.moved && difference.x.abs() <= px(4.) && difference.y.abs() <= px(4.) {
            if let Some(id) = drag.annotation {
                self.note_popup = Some((NotePopup::Personal(id), event.position));
            } else if let Some(link) = drag.link {
                self.follow(link, window, cx);
            }
        } else if self.selection.is_some() {
            self.anchor = Some(event.position);
        }
        cx.notify();
    }
}

pub(crate) fn reading_font_overrides(block: &Block) -> Vec<(Range<usize>, SharedString)> {
    block
        .spans
        .iter()
        .filter(|span| span.style.code)
        .map(|span| (span.range.clone(), SharedString::from("Menlo")))
        .collect()
}

pub(crate) fn is_note_marker_label(text: &str) -> bool {
    let text = text.trim_matches([
        '[', ']', '［', '］', '〔', '〕', '【', '】', '(', ')', '（', '）',
    ]);
    !text.is_empty()
        && text.chars().count() <= 8
        && (text
            .chars()
            .all(|ch| matches!(ch, '0'..='9' | '０'..='９' | '*' | '＊' | '†' | '‡' | '※'))
            || matches!(text, "注" | "註" | "校" | "评" | "評" | "注释" | "註釋")
            || text.len() == 1 && text.as_bytes()[0].is_ascii_alphabetic())
}

fn paint_note_markers(
    text: &str,
    layout: &TextLayout,
    markers: &[NoteMarker],
    window: &mut Window,
    cx: &mut App,
) {
    // Keep the original byte ranges and caret positions for selection and note links.
    let origin = layout.bounds().origin;
    let line_height = layout.line_height();
    let clip = window.content_mask().bounds;
    let mut font = window.text_style().font();
    font.weight = FontWeight::SEMIBOLD;
    let mut y = origin.y;
    let mut line_start = 0;
    let mut remaining = markers;
    for line in layout.line_layouts() {
        let full = &line.unwrapped_layout;
        let row_ends = line
            .wrap_boundaries()
            .iter()
            .map(|wrap| {
                let glyph = &full.runs[wrap.run_ix].glyphs[wrap.glyph_ix];
                (glyph.index, glyph.position.x)
            })
            .chain([(full.len, full.width)]);
        let mut row_byte = line_start;
        let mut row_x = px(0.);
        for (end, end_x) in row_ends {
            if y >= clip.bottom() || remaining.is_empty() {
                return;
            }
            let row_end = line_start + end;
            if y + line_height > clip.top() {
                remaining =
                    &remaining[remaining.partition_point(|note| note.range.end <= row_byte)..];
                for note in remaining
                    .iter()
                    .take_while(|note| note.range.start < row_end)
                {
                    let start = note.range.start.max(row_byte);
                    let end = note.range.end.min(row_end);
                    let Some(label) = text.get(start..end).filter(|label| !label.is_empty()) else {
                        continue;
                    };
                    let left = full.x_for_index(start - line_start);
                    let right = full.x_for_index(end - line_start);
                    let shape = window.text_system().shape_line(
                        label.to_owned().into(),
                        full.font_size * 0.68,
                        &[TextRun {
                            len: label.len(),
                            font: font.clone(),
                            color: note.color,
                            ..Default::default()
                        }],
                        None,
                    );
                    let x =
                        origin.x + left - row_x + (right - left - shape.width()).max(px(0.)) * 0.25;
                    let raised = y
                        + (full.ascent - full.descent - shape.ascent + shape.descent) / 2.
                        - full.font_size * 0.35;
                    let _ = shape.paint(
                        point(x, raised),
                        line_height,
                        TextAlign::Left,
                        None,
                        window,
                        cx,
                    );
                }
            }
            row_byte = row_end;
            row_x = end_x;
            y += line_height;
        }
        line_start += full.len + 1;
    }
}

pub(crate) fn bracketed_notes(text: &str) -> Vec<(Range<usize>, NoteKind)> {
    static BRACKETS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"[〖【〔]([^〖〗【】〔〕
：:]{1,24})[：:][^〖〗【】〔〕]*[〗】〕]",
        )
        .unwrap()
    });
    if !text.contains(['〖', '【', '〔']) {
        return vec![];
    }
    BRACKETS
        .captures_iter(text)
        .filter_map(|capture| {
            let label = &capture[1];
            let kind = if ["校记", "校勘", "异文", "校記", "異文"]
                .iter()
                .any(|word| label.contains(word))
            {
                NoteKind::Variant
            } else if [
                "夹批", "夹评", "旁批", "旁评", "眉批", "总批", "回评", "评点", "評點", "夾批",
                "夾評", "旁評", "總批", "回評",
            ]
            .iter()
            .any(|word| label.contains(word))
            {
                NoteKind::Commentary
            } else if matches!(
                label.trim(),
                "注" | "註" | "夹注" | "夾注" | "词注" | "詞註"
            ) {
                NoteKind::Gloss
            } else {
                return None;
            };
            Some((capture.get(0)?.range(), kind))
        })
        .collect()
}

fn schedule_picture(view: WeakEntity<Reader>, book: BookInfo, path: String, window: &Window) {
    window.on_next_frame(move |_, app| {
        let _ = view.update(app, |reader, cx| reader.request_picture(book, path, cx));
    });
}

fn measure_row(
    row: &mut AnyElement,
    text: Option<(TextLayout, Pixels)>,
    width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Row {
    let height = row
        .layout_as_root(
            size(AvailableSpace::Definite(width), AvailableSpace::MinContent),
            window,
            cx,
        )
        .height;
    let mut lines = Vec::new();
    if let Some((layout, top)) = text {
        let line_height = layout.line_height();
        let mut offset = top;
        let mut byte = 0;
        for line in layout.line_layouts() {
            if byte > 0 {
                lines.push(LineStart { byte, offset });
            }
            for wrap in line.wrap_boundaries() {
                offset += line_height;
                let glyph = &line.unwrapped_layout.runs[wrap.run_ix].glyphs[wrap.glyph_ix];
                lines.push(LineStart {
                    byte: byte + glyph.index,
                    offset,
                });
            }
            byte += line.len() + 1;
            offset += line_height;
        }
    }
    Row { height, lines }
}

fn nearest_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn word_at(text: &str, byte: usize) -> Range<usize> {
    let Some(ch) = text.get(byte..).and_then(|tail| tail.chars().next()) else {
        return text
            .char_indices()
            .next_back()
            .map_or(0..0, |(last, _)| last..text.len());
    };
    if ch.is_ascii_alphanumeric() {
        let start = text[..byte]
            .bytes()
            .rev()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .count();
        let len = text[byte..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .count();
        (byte - start)..(byte + len)
    } else {
        byte..(byte + ch.len_utf8())
    }
}
