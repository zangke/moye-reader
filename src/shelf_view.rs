use std::{sync::Arc, time::Duration};

use gpui_kit::{prelude::FluentBuilder as _, *};

use crate::{
    book::{BookInfo, DEMO_ID},
    demo,
    reader::{ImageState, Reader, Screen},
    shelf_folders::{FOLDER, FOLDER_PLUS},
    style::{BOOK, FILE, Palette, SERIF, icon, label},
    ui::button_style,
};

const GRID_GAP: Pixels = px(20.);
const HOLD_TIME: Duration = Duration::from_millis(470);

pub(crate) struct ShelfPress {
    book: String,
    origin: Point<Pixels>,
    activated: bool,
    _timer: Task<()>,
}

impl Reader {
    pub(crate) fn clear_shelf_selection(&mut self) {
        self.shelf_selecting = false;
        self.shelf_selected.clear();
        self.shelf_press = None;
        self.shelf_skip_click = None;
    }

    fn start_shelf_press(
        &mut self,
        book: &str,
        event: &MouseDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.shelf_skip_click = None;
        self.shelf_press = None;
        if self.shelf_selecting || self.shelf_dialog.is_some() {
            return;
        }
        let timer = cx.spawn_in(window, async move |weak, cx| {
            cx.background_executor().timer(HOLD_TIME).await;
            let _ = weak.update(cx, |reader, cx| {
                if let Some(press) = reader.shelf_press.as_mut() {
                    press.activated = true;
                    let book = press.book.clone();
                    reader.select_shelf_book(book, cx);
                }
            });
        });
        self.shelf_press = Some(ShelfPress {
            book: book.to_owned(),
            origin: event.position,
            activated: false,
            _timer: timer,
        });
    }

    pub(crate) fn shelf_press_moved(&mut self, event: &MouseMoveEvent) {
        if let Some(press) = &self.shelf_press {
            let delta = event.position - press.origin;
            if !press.activated
                && (event.pressed_button != Some(MouseButton::Left)
                    || delta.x.abs() > px(8.)
                    || delta.y.abs() > px(8.))
            {
                self.shelf_press = None;
            }
        }
    }

    pub(crate) fn finish_shelf_press(&mut self) {
        if let Some(press) = self.shelf_press.take().filter(|press| press.activated) {
            self.shelf_skip_click = Some(press.book);
        }
    }

    fn select_shelf_book(&mut self, book: String, cx: &mut Context<Self>) {
        self.shelf_selecting = true;
        self.shelf_selected.insert(book);
        cx.notify();
    }

    pub fn shelf_page(&self, window: &Window, p: Palette, cx: &Context<Self>) -> Div {
        let gutter = px(if self.handheld { 22. } else { 44. });
        let width = (window.viewport_size().width - gutter * 2.).min(px(1120.));
        let columns = ((f32::from(width + GRID_GAP) / 168.) as usize).clamp(2, 7);
        let cover_width = (width - GRID_GAP * (columns - 1) as f32) / columns as f32;
        let cover_height = cover_width * 1.42;
        let visible: Vec<_> = self
            .library
            .books
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(index, book)| {
                (self.library.book_folders.get(&book.id).copied() == self.shelf_folder)
                    .then_some(index)
            })
            .collect();
        let selectable = !visible.is_empty();
        let show_demo = self.shelf_folder.is_none();
        let count = visible.len() + usize::from(show_demo);
        let view = cx.weak_entity();
        let grid = uniform_list(
            ("shelf-grid", self.shelf_folder.unwrap_or_default()),
            count.div_ceil(columns),
            move |range, window, app| {
                let Some(entity) = view.upgrade() else {
                    return vec![];
                };
                let reader = entity.read(app);
                range
                    .map(|row| {
                        div()
                            .w_full()
                            .h(cover_height + px(54.))
                            .flex()
                            .gap(GRID_GAP)
                            .children((0..columns).map(|column| {
                                let index = row * columns + column;
                                let book = visible
                                    .get(index)
                                    .and_then(|&id| reader.library.books.get(id).cloned())
                                    .or_else(|| {
                                        (show_demo && index == visible.len()).then(demo::info)
                                    });
                                div().w(cover_width).flex_shrink_0().min_w_0().when_some(
                                    book,
                                    |tile, book| {
                                        tile.child(reader.shelf_tile(
                                            book,
                                            cover_height,
                                            p,
                                            window,
                                            &view,
                                        ))
                                    },
                                )
                            }))
                    })
                    .collect::<Vec<_>>()
            },
        )
        .w_full()
        .flex_1()
        .min_h_0();

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .items_center()
            .px(gutter)
            .child(
                div()
                    .w_full()
                    // .max_w(px(1120.)) //TODO
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .pt(px(if self.handheld { 18. } else { 28. }))
                    .gap(px(19.))
                    .child(self.shelf_header(selectable, p, cx))
                    .when(!self.library.folders.is_empty(), |layout| {
                        layout.child(self.folder_tabs(p, cx))
                    })
                    .when(count == 0, |layout| {
                        layout.child(
                            label("文件夹为空。可导入书籍，或从书库移入已有的书。", 14., p.dim)
                                .py(px(22.)),
                        )
                    })
                    .when(count > 0, |layout| layout.child(grid)),
            )
    }

    fn shelf_header(&self, selectable: bool, p: Palette, cx: &Context<Self>) -> Div {
        let folder = self.shelf_folder.and_then(|id| self.library.folder(id));
        let title = if self.shelf_selecting {
            format!("已选 {} 本", self.shelf_selected.len())
        } else {
            folder
                .map_or("书库", |folder| folder.name.as_str())
                .to_owned()
        };
        let header = div().flex().items_center().gap(px(8.)).child(
            label(title, 28., p.ink)
                .font_weight(FontWeight::SEMIBOLD)
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_ellipsis(),
        );
        if self.shelf_selecting {
            return header
                .when(!self.shelf_selected.is_empty(), |header| {
                    header.child(
                        button_style("shelf-move-selection", p.white, p.green)
                            .bg(rgb(p.button))
                            .child(icon(FOLDER, 15., p.white))
                            .child("移至文件夹")
                            .on_click(cx.listener(|reader, _, window, cx| {
                                reader.move_selected_books(window, cx)
                            })),
                    )
                })
                .child(
                    button_style("shelf-exit-selection", p.soft, p.hover)
                        .child("取消")
                        .on_click(cx.listener(|reader, _, window, cx| {
                            reader.clear_shelf_selection();
                            reader.focus.focus(window, cx);
                            cx.notify();
                        })),
                );
        }
        header
            .when_some(folder, |header, folder| {
                let id = folder.id;
                header
                    .child(
                        button_style("shelf-folder-rename", p.soft, p.hover)
                            .child("重命名")
                            .on_click(cx.listener(move |reader, _, window, cx| {
                                reader.name_shelf_folder(Some(id), window, cx)
                            })),
                    )
                    .child(
                        button_style("shelf-folder-remove", p.soft, p.hover)
                            .child("移除文件夹")
                            .on_click(cx.listener(move |reader, _, window, cx| {
                                reader.remove_shelf_folder(id, window, cx)
                            })),
                    )
            })
            .when(selectable, |header| {
                header.child(
                    button_style("shelf-select", p.soft, p.hover)
                        .child("选择")
                        .on_click(cx.listener(|reader, _, _, cx| {
                            reader.shelf_selecting = true;
                            cx.notify();
                        })),
                )
            })
            .child(
                button_style("shelf-folder-new", p.soft, p.hover)
                    .bg(rgb(p.surface))
                    .child(icon(FOLDER_PLUS, 16., p.green))
                    .child("新建文件夹")
                    .on_click(cx.listener(|reader, _, window, cx| {
                        reader.name_shelf_folder(None, window, cx)
                    })),
            )
    }

    fn folder_tabs(&self, p: Palette, cx: &Context<Self>) -> impl IntoElement {
        div()
            .id("shelf-folders")
            .flex()
            .items_center()
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .gap(px(6.))
            .overflow_x_scroll()
            .pb(px(3.))
            .children(
                std::iter::once((None, "书库首页"))
                    .chain(
                        self.library
                            .folders
                            .iter()
                            .map(|folder| (Some(folder.id), folder.name.as_str())),
                    )
                    .map(|(id, name)| {
                        let active = self.shelf_folder == id;
                        button_style(
                            ("shelf-folder", id.unwrap_or_default()),
                            if active { p.green } else { p.soft },
                            p.hover,
                        )
                        .max_w(px(200.))
                        .bg(rgb(if active { p.green_bg } else { p.surface }))
                        .child(icon(FOLDER, 15., if active { p.green } else { p.dim }))
                        .child(
                            label(name.to_owned(), 12., if active { p.green } else { p.soft })
                                .min_w_0()
                                .whitespace_nowrap()
                                .text_ellipsis(),
                        )
                        .on_click(cx.listener(
                            move |reader, _, window, cx| reader.show_shelf_folder(id, window, cx),
                        ))
                    }),
            )
    }

    fn shelf_tile(
        &self,
        book: BookInfo,
        height: Pixels,
        p: Palette,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> impl IntoElement {
        let stored = book.id != DEMO_ID;
        let selected = self.shelf_selected.contains(&book.id);
        let progress = self
            .library
            .reading
            .get(&book.id)
            .map_or(0., |reading| reading.position.progress)
            .clamp(0., 1.);
        let caption = if progress >= 0.995 {
            "已读完".to_owned()
        } else if progress > 0. {
            format!("已读 {:.0}%", (progress * 100.).max(1.))
        } else if !stored {
            "内置试读".to_owned()
        } else if self.library.active.as_deref() == Some(book.id.as_str()) {
            "阅读中".to_owned()
        } else {
            "未读".to_owned()
        };
        let artwork = if stored {
            self.cover_image(&book, window, view)
        } else {
            self.shelf_scene.clone()
        };
        let image = match artwork {
            Some(image) => div()
                .size_full()
                .relative()
                .child(img(image).size_full().object_fit(if stored {
                    ObjectFit::Contain
                } else {
                    ObjectFit::Cover
                }))
                .when(!stored, |cover| {
                    cover.child(
                        div()
                            .absolute()
                            .inset_0()
                            .bg(rgba(0x14282566))
                            .p(px(15.))
                            .flex()
                            .items_end()
                            .child(
                                label(book.title.clone(), 22., 0xf4edd5)
                                    .font_family(SERIF)
                                    .font_weight(FontWeight::SEMIBOLD),
                            ),
                    )
                }),
            None => {
                let (background, ink, stripe) = match book.format {
                    crate::book::Format::Txt => (0x314a45, 0xf0e9d7, 0x9bb9a4),
                    crate::book::Format::Epub => (0x45444e, 0xf1ebdd, 0xaca4bc),
                };
                div()
                    .size_full()
                    .bg(rgb(background))
                    .flex()
                    .child(div().w(px(5.)).h_full().flex_shrink_0().bg(rgb(stripe)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .p(px(15.))
                            .pt(px(21.))
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .child(
                                label(book.title.clone(), 20., ink)
                                    .font_family(SERIF)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .line_height(px(26.))
                                    .line_clamp(4),
                            )
                            .when(!book.author.is_empty(), |cover| {
                                cover.child(label(book.author.clone(), 12., ink).line_clamp(2))
                            }),
                    )
            }
        };
        let open = view.clone();
        let press = view.clone();
        let press_book = book.id.clone();
        let stable_id = SharedString::from(book.id.clone());
        let cover = div()
            .id(stable_id)
            .relative()
            .w_full()
            .h(height)
            .rounded(px(8.))
            .overflow_hidden()
            .bg(rgb(p.panel))
            .border_1()
            .border_color(rgb(if selected { p.green } else { p.line }))
            .hover(|cover| cover.border_color(rgb(p.green)))
            .focus_visible(|cover| cover.border_color(rgb(p.green)))
            .opacity(if self.shelf_selecting && !stored {
                0.5
            } else {
                1.
            })
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .aria_label(format!(
                "{}{}",
                if self.shelf_selecting {
                    "选择 "
                } else {
                    "阅读 "
                },
                book.title
            ))
            .child(image)
            .when(self.shelf_selecting && stored, |cover| {
                cover.child(
                    div()
                        .absolute()
                        .top(px(8.))
                        .right(px(8.))
                        .size(px(23.))
                        .rounded(px(5.))
                        .bg(rgb(if selected { p.button } else { p.panel }))
                        .border_1()
                        .border_color(rgb(if selected { p.button } else { p.soft }))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(selected, |check| {
                            check.child(icon("icons/check.svg", 16., p.white))
                        }),
                )
            })
            .when(stored, |cover| {
                cover
                    .child(touch_select(book.id.clone(), view.clone()))
                    .on_mouse_down(MouseButton::Left, move |event, window, app| {
                        let _ = press.update(app, |reader, cx| {
                            reader.start_shelf_press(&press_book, event, window, cx)
                        });
                    })
            })
            .on_click(move |event, window, app| {
                let _ = open.update(app, |reader, cx| {
                    if matches!(event, ClickEvent::Mouse(_))
                        && reader.shelf_skip_click.take().as_deref() == Some(book.id.as_str())
                    {
                        return;
                    }
                    if reader.shelf_selecting {
                        if stored {
                            if !reader.shelf_selected.remove(&book.id) {
                                reader.shelf_selected.insert(book.id.clone());
                            }
                            cx.notify();
                        }
                    } else {
                        reader.switch_book(book.clone(), window, cx);
                    }
                });
            });
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(7.))
            .child(cover)
            .child(
                div()
                    .w_full()
                    .h(px(2.))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(p.line))
                    .when(progress > 0., |track| {
                        track.child(div().h_full().w(relative(progress)).bg(rgb(p.green)))
                    }),
            )
            .child(label(caption, 11., p.dim))
    }

    fn cover_image(
        &self,
        info: &BookInfo,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> Option<Arc<RenderImage>> {
        let path = info.cover.as_deref()?;
        match self.picture(info, path) {
            Some(ImageState::Ready(image)) => Some(image.art.clone()),
            Some(_) => None,
            None => {
                let info = info.clone();
                let path = path.to_owned();
                let view = view.clone();
                window.on_next_frame(move |_, app| {
                    let _ = view.update(app, |reader, cx| reader.request_picture(info, path, cx));
                });
                None
            }
        }
    }
    pub(crate) fn book_cover(
        &self,
        info: &BookInfo,
        p: Palette,
        window: &Window,
        view: &WeakEntity<Self>,
    ) -> Div {
        let base = div()
            .relative()
            .w(px(49.))
            .h(px(72.))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(px(5.))
            .border_1()
            .border_color(rgb(p.line));
        if let Some(image) = self.cover_image(info, window, view) {
            return base.child(img(image).size_full().object_fit(ObjectFit::Cover));
        }
        if info.id == DEMO_ID {
            base.bg(rgb(0x2f4a49))
                .when_some(self.shelf_scene.as_ref(), |cover, image| {
                    cover.child(
                        img(image.clone())
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .child(div().absolute().inset_0().bg(rgba(0x15282950)))
                .child(
                    label("雾岸\n来信", 13., 0xf4edd5)
                        .line_height(px(15.))
                        .font_family(SERIF)
                        .font_weight(FontWeight::BOLD)
                        .absolute()
                        .left(px(6.))
                        .top(px(8.)),
                )
        } else {
            base.bg(rgb(p.green_bg))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    if info.format == crate::book::Format::Txt {
                        FILE
                    } else {
                        BOOK
                    },
                    20.,
                    p.green,
                ))
        }
    }
}

fn touch_select(book: String, view: WeakEntity<Reader>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let visible = bounds.intersect(&window.content_mask().bounds);
            window.on_mouse_event(move |event: &LongPressEvent, phase, window, app| {
                if phase.bubble()
                    && event.phase == TouchPhase::Started
                    && !window.default_prevented()
                    && visible.contains(&event.start_position)
                {
                    let active = view
                        .update(app, |reader, cx| {
                            if reader.screen != Screen::Library
                                || reader.shelf_dialog.is_some()
                                || reader.sources.is_some()
                            {
                                return false;
                            }
                            reader.select_shelf_book(book.clone(), cx);
                            true
                        })
                        .unwrap_or(false);
                    if active {
                        window.prevent_default();
                        app.stop_propagation();
                    }
                }
            });
        },
    )
    .absolute()
    .inset_0()
}
