use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use gpui_kit::{
    component::{Theme, ThemeMode, input::TextareaState},
    *,
};

use crate::{
    book::{Book, BookInfo, DEMO_ID, Format, Link, Passage, PointInText, Target},
    demo,
    library::{Annotation, Bookmark, Library, Paper, Position, ReadingMode, Store, Tone},
    pagination::{Anchor, Pagination, page_range},
    pdf,
    pictures::{Picture, read_picture, read_shelf_scene},
    plugin_view::{PluginPage, SourcesPanel},
    shelf_folders::ShelfDialog,
    shelf_view::ShelfPress,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Source,
    Personal,
    Bookmarks,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Library,
    Reading,
    Plugins,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReadingPanel {
    Contents,
    Notes,
}

pub enum NotePopup {
    Source(Target),
    Personal(u64),
}

#[derive(Clone)]
pub struct Editor {
    pub id: Option<u64>,
    pub passage: Passage,
    pub quote: String,
    pub tone: Tone,
}

pub struct Drag {
    pub start: PointInText,
    pub end: PointInText,
    pub origin: Point<Pixels>,
    pub link: Option<Link>,
    pub annotation: Option<u64>,
    pub moved: bool,
}

pub enum ImageState {
    Loading,
    Ready(Picture),
    Failed,
}

pub struct Reader {
    pub book: Arc<Book>,
    pub library: Library,
    pub section: usize,
    pub list: ListState,
    pub pagination: Option<Pagination>,
    pub page_anchor: Anchor,
    pub page_index: usize,
    pub pending_page_end: bool,
    pub pending_page_offset: Option<Pixels>,
    pub pending_scroll_anchor: Option<Anchor>,
    pub source_list: ListState,
    pub screen: Screen,
    pub handheld: bool,
    pub reading_drawer: Option<ReadingPanel>,
    pub handheld_controls: FocusHandle,
    pub contents_scroll: UniformListScrollHandle,
    pub shelf_scene: Option<Arc<RenderImage>>,
    pub(crate) shelf_folder: Option<u64>,
    pub(crate) shelf_dialog: Option<ShelfDialog>,
    pub(crate) shelf_selecting: bool,
    pub(crate) shelf_selected: HashSet<String>,
    pub(crate) shelf_press: Option<ShelfPress>,
    pub(crate) shelf_skip_click: Option<String>,
    pub tab: Tab,
    pub source: Option<Target>,
    pub note_popup: Option<(NotePopup, Point<Pixels>)>,
    pub titlebar_press: Option<Point<Pixels>>,
    pub drag: Option<Drag>,
    pub selection: Option<Passage>,
    pub anchor: Option<Point<Pixels>>,
    pub chosen_tone: Tone,
    pub revealed: Option<u64>,
    pub unlocated_annotations: HashSet<u64>,
    pub settings_open: bool,
    pub editor: Option<Editor>,
    pub input: Entity<TextareaState>,
    pub focus: FocusHandle,
    pub notice: Option<(String, bool)>,
    pub busy: bool,
    pub(crate) sources: Option<SourcesPanel>,
    pub(crate) plugin_page: Option<PluginPage>,
    picking: bool,
    pictures: HashMap<String, HashMap<String, ImageState>>,
    pub(crate) store: Option<Store>,
    dirty: bool,
    save_task: Option<Task<()>>,
    notice_task: Option<Task<()>>,
    image_fifo: VecDeque<(String, String)>,
    back: Vec<Position>,
}

impl Reader {
    pub fn new(
        files: Vec<PathBuf>,
        handheld: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (store, library, notice) =
            match Store::new().and_then(|store| store.load().map(|library| (store, library))) {
                Ok((store, library)) => (Some(store), library, None),
                Err(error) => (
                    None,
                    Library::default(),
                    Some((format!("无法载入本地书库：{error}"), true)),
                ),
            };
        let last = library
            .active
            .as_ref()
            .filter(|id| id.as_str() != DEMO_ID)
            .and_then(|id| library.books.iter().find(|info| &info.id == id))
            .cloned();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(4)
                .placeholder("写下你的想法…")
        });
        let weak = cx.weak_entity();
        let list = ListState::new(0, ListAlignment::Top, px(360.));
        list.set_scroll_handler(move |_, window, _| {
            let weak = weak.clone();
            window.on_next_frame(move |window, cx| {
                let _ = weak.update(cx, |reader, cx| reader.scrolled(window, cx));
            });
        });

        let mut reader = Self {
            book: Arc::new(demo::book()),
            library,
            section: 0,
            list,
            pagination: None,
            page_anchor: Anchor::default(),
            page_index: 0,
            pending_page_end: false,
            pending_page_offset: None,
            pending_scroll_anchor: None,
            source_list: ListState::new(0, ListAlignment::Top, px(180.)),
            screen: Screen::Library,
            handheld,
            reading_drawer: None,
            handheld_controls: cx.focus_handle(),
            contents_scroll: UniformListScrollHandle::new(),
            shelf_scene: None,
            shelf_folder: None,
            shelf_dialog: None,
            shelf_selecting: false,
            shelf_selected: HashSet::new(),
            shelf_press: None,
            shelf_skip_click: None,
            tab: Tab::Source,
            source: None,
            note_popup: None,
            titlebar_press: None,
            drag: None,
            selection: None,
            anchor: None,
            chosen_tone: Tone::Amber,
            revealed: None,
            unlocated_annotations: HashSet::new(),
            settings_open: false,
            editor: None,
            input,
            focus,
            notice,
            busy: false,
            sources: None,
            plugin_page: None,
            picking: false,
            pictures: HashMap::new(),
            store,
            dirty: false,
            save_task: None,
            notice_task: None,
            image_fifo: VecDeque::new(),
            back: vec![],
        };
        reader.restore_annotation_links();
        let initial = reader.saved_position();
        reader.show_position(initial, cx);
        if !files.is_empty() {
            reader.import_paths(files, window, cx);
        } else if let Some(info) = last {
            reader.load_book(info, false, window, cx);
        }
        cx.spawn(async move |weak, cx| {
            let scene = cx.background_spawn(async { read_shelf_scene() }).await;
            if let Ok(scene) = scene {
                let _ = weak.update(cx, |reader, cx| {
                    reader.shelf_scene = Some(scene.art);
                    cx.notify();
                });
            }
        })
        .detach();
        cx.on_app_quit(|reader, _| {
            reader.sources = None;
            reader.plugin_page = None;
            reader.capture_position();
            reader.flush();
            async {}
        })
        .detach();
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, app| {
            let _ = weak.update(app, |reader, _| {
                reader.sources = None;
                reader.plugin_page = None;
                reader.capture_position();
                reader.flush();
            });
            true
        });
        reader
    }

    pub fn data(&self) -> &crate::book::Section {
        &self.book.sections[self.section]
    }

    pub fn annotations(&self) -> &[Annotation] {
        self.library
            .reading
            .get(&self.book.info.id)
            .map_or(&[], |reading| reading.annotations.as_slice())
    }

    pub fn saved_position(&self) -> Position {
        self.library.reading.get(&self.book.info.id).map_or_else(
            || Position {
                section: self.book.linear[0],
                ..Default::default()
            },
            |reading| reading.position,
        )
    }

    pub fn is_paged(&self) -> bool {
        self.reading_mode() != ReadingMode::Scroll
    }

    pub fn is_spread(&self) -> bool {
        self.reading_mode() == ReadingMode::Spread
    }

    pub fn reading_mode(&self) -> ReadingMode {
        if self.handheld {
            self.library.settings.handheld_reading_mode
        } else {
            self.library.settings.reading_mode
        }
    }

    pub fn panel_open(&self, panel: ReadingPanel) -> bool {
        if panel == ReadingPanel::Contents && !self.handheld {
            self.library.settings.sidebar_open
        } else {
            self.reading_drawer == Some(panel)
        }
    }

    pub fn set_panel(&mut self, panel: ReadingPanel, open: bool, cx: &mut Context<Self>) {
        if panel == ReadingPanel::Contents && !self.handheld {
            self.library.settings.sidebar_open = open;
            self.save_now(cx);
        } else {
            if open {
                self.reading_drawer = Some(panel);
                self.settings_open = false;
                self.note_popup = None;
                self.anchor = None;
            } else if self.reading_drawer == Some(panel) {
                self.reading_drawer = None;
            }
            cx.notify();
        }
        if open
            && panel == ReadingPanel::Contents
            && let Some(index) =
                self.contents_entry(self.section, self.visible_item().saturating_sub(1))
        {
            self.contents_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    pub fn pages_per_turn(&self) -> usize {
        if self.is_spread() { 2 } else { 1 }
    }

    pub fn visible_pages(&self) -> std::ops::Range<usize> {
        page_range(self.page_index, self.page_count(), self.pages_per_turn())
    }

    pub fn page_count(&self) -> usize {
        self.pagination
            .as_ref()
            .map_or(1, |layout| layout.pages.len())
    }

    pub fn can_turn(&self, forward: bool) -> bool {
        if !self.is_paged() {
            return self.chapter_neighbor(forward).is_some();
        }
        let pages = self.visible_pages();
        let in_section = if forward {
            pages.end < self.page_count()
        } else {
            pages.start > 0
        };
        in_section || self.neighbor(forward).is_some()
    }

    pub fn visible_item(&self) -> usize {
        if self.is_paged() {
            self.page_anchor.item
        } else {
            self.list.logical_scroll_top().item_ix
        }
    }

    fn current_position(&self) -> Position {
        let (anchor, offset, at_end) = if self.is_paged() {
            let offset = self
                .pagination
                .as_ref()
                .and_then(|layout| layout.rows.get(self.page_anchor.item))
                .map_or(px(0.), |row| row.offset_for(self.page_anchor.byte));
            (
                self.page_anchor,
                offset,
                self.visible_pages().end == self.page_count(),
            )
        } else {
            let pos = self.list.logical_scroll_top();
            let at_end = self.list.is_scrolled_to_end() == Some(true)
                && (pos.item_ix > 0 || pos.offset_in_item > px(0.));
            (
                Anchor {
                    item: pos.item_ix,
                    byte: 0,
                },
                pos.offset_in_item,
                at_end,
            )
        };
        Position {
            section: self.section,
            item: anchor.item,
            byte: anchor.byte,
            offset: offset.into(),
            progress: self
                .book
                .progress(self.section, anchor.item, anchor.byte, at_end),
        }
    }

    pub fn progress(&self) -> f32 {
        self.current_position().progress
    }

    pub fn capture_position(&mut self) {
        if self.busy
            || self.screen != Screen::Reading
            || (self.is_paged()
                && (self.pagination.is_none()
                    || self.pending_page_end
                    || self.pending_page_offset.is_some()))
            || self.pending_scroll_anchor.is_some()
        {
            return;
        }
        let position = self.current_position();
        let reading = self
            .library
            .reading
            .entry(self.book.info.id.clone())
            .or_default();
        if position != reading.position {
            reading.position = position;
            self.dirty = true;
        }
    }

    fn scrolled(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.is_paged() {
            return;
        }
        self.note_popup = None;
        self.capture_position();
        self.schedule_save(window, cx);
        cx.notify();
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        if let Some(store) = self.store.as_ref() {
            match store.save(&self.library) {
                Ok(()) => self.dirty = false,
                Err(error) => self.notice = Some((format!("书库保存失败：{error}"), true)),
            }
        } else {
            self.notice = Some((
                "本地书库不可用，当前改动仅保留在这次会话中".to_owned(),
                true,
            ));
        }
    }

    pub fn schedule_save(&mut self, window: &Window, cx: &mut Context<Self>) {
        if !self.dirty {
            return;
        }
        self.save_task = Some(cx.spawn_in(window, async move |weak, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let _ = weak.update(cx, |reader, cx| {
                reader.flush();
                if reader.dirty {
                    cx.notify();
                }
            });
        }));
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self.dirty = true;
        self.flush();
        cx.notify();
    }

    pub fn tell(
        &mut self,
        text: impl Into<String>,
        error: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.notice = Some((text.into(), error));
        self.notice_task = if error {
            None
        } else {
            Some(cx.spawn_in(window, async move |weak, cx| {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                let _ = weak.update(cx, |reader, cx| {
                    if reader.notice.as_ref().is_some_and(|(_, error)| !error) {
                        reader.notice = None;
                        cx.notify();
                    }
                });
            }))
        };
        cx.notify();
    }

    pub fn choose_files(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.busy || self.picking {
            return;
        }
        self.picking = true;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("导入 TXT、EPUB 或 PDF".into()),
        });
        cx.spawn_in(window, async move |weak, cx| {
            let result = paths.await;
            let _ = weak.update_in(cx, |reader, window, cx| {
                reader.picking = false;
                match result {
                    Ok(Ok(Some(paths))) => reader.import_paths(paths, window, cx),
                    Ok(Err(error)) => {
                        reader.tell(format!("无法打开文件选择器：{error}"), true, window, cx)
                    }
                    _ => {}
                }
            });
        })
        .detach();
    }

    pub fn import_paths(&mut self, paths: Vec<PathBuf>, window: &Window, cx: &mut Context<Self>) {
        self.import_files(paths, None, window, cx);
    }

    pub(crate) fn import_files(
        &mut self,
        paths: Vec<PathBuf>,
        download_dir: Option<tempfile::TempDir>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.busy {
            return;
        }
        let Some(store) = self.store.clone() else {
            self.tell("本地书库不可用，无法导入", true, window, cx);
            return;
        };
        self.capture_position();
        self.sources = None;
        let folder = (self.screen == Screen::Library)
            .then_some(self.shelf_folder)
            .flatten();
        self.busy = true;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |weak, cx| {
            let (mut books, last, errors) = cx
                .background_spawn(async move {
                    let _download_dir = download_dir;
                    let mut books = vec![];
                    let mut last = None;
                    let mut errors = vec![];
                    for path in paths {
                        match store.import(&path) {
                            Ok(book) => {
                                books.push(book.info.clone());
                                last = Some(book);
                            }
                            Err(error) => {
                                let name = path.file_name().unwrap_or_default().to_string_lossy();
                                errors.push(format!("{name}：{error}"));
                            }
                        }
                    }
                    (books, last, errors)
                })
                .await;
            let _ = weak.update_in(cx, |reader, window, cx| {
                reader.busy = false;
                let existing: std::collections::HashSet<_> = reader
                    .library
                    .books
                    .iter()
                    .map(|item| item.id.clone())
                    .collect();
                let mut seen = existing;
                books.retain(|book| seen.insert(book.id.clone()));
                if let Some(folder) = folder.filter(|id| reader.library.folder(*id).is_some()) {
                    reader
                        .library
                        .book_folders
                        .extend(books.iter().map(|book| (book.id.clone(), folder)));
                }
                reader.library.books.extend(books);
                if let Some(book) = last {
                    reader.activate(book, true, window, cx);
                }
                if !errors.is_empty() {
                    reader.tell(errors.join("\n"), true, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_library(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Library {
            if self.shelf_folder.is_some() {
                self.show_shelf_folder(None, window, cx);
            }
            return;
        }
        self.capture_position();
        self.flush();
        self.set_screen(Screen::Library, window);
        Theme::change(ThemeMode::Dark, None, cx);
        self.close_reader_overlays();
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn open_reader(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_screen(Screen::Reading, window);
        Theme::change(theme(self.library.settings.paper), None, cx);
        self.focus.focus(window, cx);
        if !self.busy && self.library.active.as_deref() != Some(self.book.info.id.as_str()) {
            self.library.active = Some(self.book.info.id.clone());
            self.save_now(cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn set_screen(&mut self, screen: Screen, window: &mut Window) {
        if self.screen != screen {
            self.sources = None;
            self.plugin_page = None;
            self.shelf_dialog = None;
            self.clear_shelf_selection();
            self.screen = screen;
            if cfg!(target_os = "macos") {
                window.set_traffic_light_position(crate::ui::traffic_light_position(screen));
            }
        }
    }

    fn close_reader_overlays(&mut self) {
        self.reading_drawer = None;
        self.note_popup = None;
        self.settings_open = false;
        self.selection = None;
        self.drag = None;
        self.anchor = None;
    }

    pub fn switch_book(&mut self, info: BookInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.load_book(info, true, window, cx);
    }

    fn load_book(
        &mut self,
        info: BookInfo,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        if info.id == self.book.info.id {
            if open {
                self.open_reader(window, cx);
            }
            return;
        }
        self.capture_position();
        if info.id == DEMO_ID {
            self.activate(demo::book(), open, window, cx);
            return;
        }
        let Some(store) = self.store.clone() else {
            return;
        };
        self.busy = true;
        self.notice = None;
        cx.notify();
        cx.spawn_in(window, async move |weak, cx| {
            let result = cx
                .background_spawn(async move { store.read_book(info) })
                .await;
            let _ = weak.update_in(cx, |reader, window, cx| {
                reader.busy = false;
                match result {
                    Ok(book) => reader.activate(book, open, window, cx),
                    Err(error) => reader.tell(format!("无法打开书籍：{error:#}"), true, window, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn activate(&mut self, book: Book, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.library.active = Some(book.info.id.clone());
        if let Some(info) = self
            .library
            .books
            .iter_mut()
            .find(|info| info.id == book.info.id)
        {
            *info = book.info.clone();
        }
        self.book = Arc::new(book);
        self.library
            .reading
            .entry(self.book.info.id.clone())
            .or_default()
            .migrate_glyph_anchors(&self.book);
        self.pagination = None;
        self.restore_annotation_links();
        self.back.clear();
        self.contents_scroll = UniformListScrollHandle::new();
        self.close_reader_overlays();
        if open {
            self.set_screen(Screen::Reading, window);
        }
        if self.screen == Screen::Reading {
            Theme::change(theme(self.library.settings.paper), None, cx);
        }
        self.tab = Tab::Source;
        self.editor = None;
        self.focus.focus(window, cx);
        self.show_position(self.saved_position(), cx);
        self.save_now(cx);
        if !self.dirty && self.book.skipped_sections > 0 {
            self.tell(
                format!("有 {} 个 EPUB 资源无法读取", self.book.skipped_sections),
                true,
                window,
                cx,
            );
        }
    }

    pub fn contents_entry(&self, section: usize, block: usize) -> Option<usize> {
        self.book.contents_index(section, block)
    }

    pub fn chapter_title(&self) -> SharedString {
        self.book
            .title_at(self.section, self.visible_item().saturating_sub(1))
    }

    pub fn page_count_is_chapter(&self) -> bool {
        if self.book.info.format == Format::Pdf {
            return false;
        }
        let Some(index) = self.contents_entry(self.section, 0) else {
            return false;
        };
        let entry = &self.book.contents[index];
        entry.section == self.section
            && entry.block == 0
            && self.contents_entry(self.section, self.data().blocks.len().saturating_sub(1))
                == Some(index)
            && self
                .neighbor(true)
                .is_none_or(|next| self.contents_entry(next, 0) != Some(index))
    }

    fn show_position(&mut self, pos: Position, cx: &mut Context<Self>) {
        let section = if pos.section < self.book.sections.len()
            && !self.book.sections[pos.section].blocks.is_empty()
        {
            pos.section
        } else {
            self.book.linear[0]
        };
        if section != self.section {
            self.pagination = None;
        }
        self.section = section;
        self.selection = None;
        self.drag = None;
        self.anchor = None;
        self.source = None;
        self.note_popup = None;
        self.revealed = None;
        let item = pos.item.min(self.data().blocks.len());
        let byte = item
            .checked_sub(1)
            .and_then(|index| self.data().blocks.get(index))
            .map_or(0, |block| pos.byte.min(block.text.len()));
        self.page_anchor = Anchor { item, byte };
        self.page_index = self
            .pagination
            .as_ref()
            .map_or(0, |layout| layout.page_for(self.page_anchor));
        self.pending_page_end = false;
        self.pending_page_offset =
            (self.is_paged() && byte == 0 && pos.offset > 0.).then_some(px(pos.offset));
        self.pending_scroll_anchor = (!self.is_paged() && byte > 0).then_some(self.page_anchor);
        self.list.reset(self.data().blocks.len() + 2);
        self.source_list.reset(self.data().notes.len());
        self.list.scroll_to(ListOffset {
            item_ix: item,
            offset_in_item: px(pos.offset.max(0.)),
        });
        if let Some(index) = self.contents_entry(self.section, pos.item.saturating_sub(1)) {
            self.contents_scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub fn navigate(&mut self, section: usize, block: usize, cx: &mut Context<Self>) {
        let item = if block == 0 || self.book.sections[section].header == Some(block) {
            0
        } else {
            block + 1
        };
        self.jump(
            Position {
                section,
                item,
                ..Default::default()
            },
            cx,
        );
    }

    pub fn navigate_to_text(&mut self, section: usize, point: PointInText, cx: &mut Context<Self>) {
        if !self.is_paged() || self.book.sections[section].header == Some(point.block) {
            self.navigate(section, point.block, cx);
        } else {
            self.jump(
                Position {
                    section,
                    item: point.block + 1,
                    byte: point.byte,
                    ..Default::default()
                },
                cx,
            );
        }
    }

    fn jump(&mut self, position: Position, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        self.show_position(position, cx);
        self.reading_drawer = None;
        self.capture_position();
        self.save_now(cx);
    }

    pub fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some((section, block)) = self.chapter_neighbor(forward) {
            self.navigate(section, block, cx);
        }
    }

    fn chapter_neighbor(&self, forward: bool) -> Option<(usize, usize)> {
        self.book
            .neighboring_chapter(self.section, self.visible_item().saturating_sub(1), forward)
            .map(|entry| (entry.section, entry.block))
    }

    pub fn turn(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        if !self.is_paged() {
            self.step(forward, cx);
            return;
        }
        let pages = self.visible_pages();
        let index = if forward {
            Some(pages.end)
        } else {
            pages.start.checked_sub(self.pages_per_turn())
        };
        let Some(layout) = self.pagination.as_ref() else {
            return;
        };
        if let Some(index) = index
            && let Some(fragment) = layout.pages.get(index).and_then(|page| page.first())
        {
            self.page_anchor = fragment.start;
            self.page_index = index;
            self.close_reader_overlays();
            self.anchor = None;
            self.capture_position();
            self.save_now(cx);
        } else if let Some(section) = self.neighbor(forward) {
            self.show_position(
                Position {
                    section,
                    ..Default::default()
                },
                cx,
            );
            self.pending_page_end = !forward;
        }
    }

    pub fn neighbor(&self, forward: bool) -> Option<usize> {
        let next = match self.book.linear.binary_search(&self.section) {
            Ok(index) if forward => index + 1,
            Ok(index) | Err(index) if !forward => index.checked_sub(1)?,
            Ok(index) | Err(index) => index,
        };
        self.book.linear.get(next).copied()
    }

    pub fn follow(&mut self, link: Link, window: &Window, cx: &mut Context<Self>) {
        match link {
            Link::Local(target) if self.book.notes.contains_key(&target) => {
                self.note_popup = Some((NotePopup::Source(target), window.mouse_position()));
                cx.notify();
            }
            Link::Local(target) => {
                if let Some(&(section, block)) = self.book.destinations.get(&target)
                    && !self.book.sections[section].blocks.is_empty()
                {
                    self.capture_position();
                    self.back.push(self.saved_position());
                    self.navigate(section, block, cx);
                } else {
                    self.tell("书中未找到这个链接的目标", true, window, cx);
                }
            }
            Link::External(url) => cx.open_url(&url),
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        if let Some(pos) = self.back.pop() {
            self.show_position(pos, cx);
            self.capture_position();
            self.save_now(cx);
        }
    }

    fn restore_annotation_links(&mut self) {
        self.unlocated_annotations.clear();
        if let Some(reading) = self.library.reading.get_mut(&self.book.info.id) {
            for note in &mut reading.annotations {
                if let Some(passage) = self.book.restore_passage(&note.passage, &note.quote) {
                    if note.passage != passage {
                        note.passage = passage;
                        self.dirty = true;
                    }
                } else {
                    self.unlocated_annotations.insert(note.id);
                }
            }
        }
    }

    pub fn bookmarks(&self) -> &[Bookmark] {
        self.library
            .reading
            .get(&self.book.info.id)
            .map_or(&[], |reading| reading.bookmarks.as_slice())
    }

    pub fn bookmark_position(&self) -> Bookmark {
        let (item, byte) = self.selection.as_ref().map_or_else(
            || (self.visible_item(), self.page_anchor.byte),
            |passage| (passage.start.block + 1, passage.start.byte),
        );
        Bookmark {
            section: self.section,
            item: item.min(self.data().blocks.len()),
            byte: if self.is_paged() { byte } else { 0 },
        }
    }

    pub fn toggle_bookmark(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        let mark = self.bookmark_position();
        if self.bookmarks().contains(&mark) {
            self.delete_bookmark(mark, cx);
            if !self.dirty {
                self.tell("已移除书签", false, window, cx);
            }
        } else {
            self.capture_position();
            self.library
                .reading
                .entry(self.book.info.id.clone())
                .or_default()
                .bookmarks
                .push(mark);
            self.tab = Tab::Bookmarks;
            self.save_now(cx);
            if !self.dirty {
                self.tell("已添加书签", false, window, cx);
            }
        }
    }

    pub fn delete_bookmark(&mut self, mark: Bookmark, cx: &mut Context<Self>) {
        if let Some(reading) = self.library.reading.get_mut(&self.book.info.id) {
            reading.bookmarks.retain(|bookmark| *bookmark != mark);
            self.save_now(cx);
        }
    }

    pub fn jump_to_bookmark(&mut self, mark: Bookmark, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() || mark.section >= self.book.sections.len() {
            return;
        }
        self.jump(
            Position {
                section: mark.section,
                item: mark.item,
                byte: mark.byte,
                ..Default::default()
            },
            cx,
        );
    }

    pub fn copy_selection(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(passage) = &self.selection {
            let quote = self.book.quote(passage);
            if !quote.trim().is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(quote));
                self.tell("已复制原文", false, window, cx);
            }
        }
    }

    pub fn add_underline(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(passage) = self.selection.take() else {
            return;
        };
        let quote = self.book.quote(&passage);
        if quote.trim().is_empty() {
            return;
        }
        self.anchor = None;
        self.put_annotation(passage, quote, String::new(), self.chosen_tone, None, cx);
        if !self.dirty {
            self.tell("已保存划线", false, window, cx);
        }
    }

    fn put_annotation(
        &mut self,
        passage: Passage,
        quote: String,
        body: String,
        tone: Tone,
        id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let reading = self
            .library
            .reading
            .entry(self.book.info.id.clone())
            .or_default();
        if let Some(note) = reading
            .annotations
            .iter_mut()
            .find(|note| Some(note.id) == id)
        {
            note.body = body;
            note.tone = tone;
        } else if let Some(note) = reading
            .annotations
            .iter_mut()
            .find(|note| note.passage == passage)
        {
            if !body.is_empty() {
                note.body = body;
            }
            note.tone = tone;
        } else {
            self.library.next_annotation += 1;
            reading.annotations.push(Annotation::new(
                self.library.next_annotation,
                passage,
                quote,
                body,
                tone,
            ));
        }
        self.tab = Tab::Personal;
        self.save_now(cx);
    }

    pub fn edit_annotation(
        &mut self,
        id: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = if let Some(id) = id {
            let Some(note) = self.annotations().iter().find(|note| note.id == id) else {
                return;
            };
            let editor = Editor {
                id: Some(id),
                passage: note.passage.clone(),
                quote: note.quote.clone(),
                tone: note.tone,
            };
            let body = note.body.clone();
            self.input
                .update(cx, |input, cx| input.set_value(body, window, cx));
            editor
        } else {
            let Some(passage) = self.selection.clone() else {
                return;
            };
            let quote = self.book.quote(&passage);
            if quote.trim().is_empty() {
                return;
            }
            if let Some(id) = self
                .annotations()
                .iter()
                .find(|note| note.passage == passage)
                .map(|note| note.id)
            {
                return self.edit_annotation(Some(id), window, cx);
            }
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            Editor {
                id: None,
                passage,
                quote,
                tone: self.chosen_tone,
            }
        };
        self.anchor = None;
        self.settings_open = false;
        self.editor = Some(editor);
        cx.on_next_frame(window, |reader, window, cx| {
            reader.input.update(cx, |input, cx| input.focus(window, cx));
        });
        cx.notify();
    }

    pub fn close_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub fn commit_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.input.read(cx).value().trim().to_owned();
        let Some(editor) = &self.editor else { return };
        if body.is_empty() && editor.id.is_none() {
            return;
        }
        let editor = editor.clone();
        self.put_annotation(
            editor.passage,
            editor.quote,
            body,
            editor.tone,
            editor.id,
            cx,
        );
        self.selection = None;
        self.close_editor(window, cx);
    }

    pub fn delete_annotation(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(reading) = self.library.reading.get_mut(&self.book.info.id) {
            reading.annotations.retain(|note| note.id != id);
            self.revealed = None;
            self.save_now(cx);
        }
    }

    pub fn reveal_annotation(&mut self, id: u64, window: &Window, cx: &mut Context<Self>) {
        if self.unlocated_annotations.contains(&id) {
            self.tell("当前排版中找不到这条批注的原文位置。", true, window, cx);
            return;
        }
        if let Some(passage) = self
            .annotations()
            .iter()
            .find(|note| note.id == id)
            .map(|note| note.passage.clone())
        {
            self.navigate_to_text(passage.section, passage.start, cx);
            self.revealed = Some(id);
            cx.notify();
        }
    }

    pub fn set_paper(&mut self, paper: Paper, cx: &mut Context<Self>) {
        self.library.settings.paper = paper;
        Theme::change(theme(paper), None, cx);
        self.save_now(cx);
    }

    pub fn set_reading_mode(
        &mut self,
        mode: ReadingMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.reading_mode() != mode && !self.busy && self.editor.is_none() {
            let position = self.current_position();
            if self.handheld {
                self.library.settings.handheld_reading_mode = mode;
            } else {
                self.library.settings.reading_mode = mode;
            }
            self.pagination = None;
            self.show_position(position, cx);
            self.focus.focus(window, cx);
            self.save_now(cx);
        }
    }

    pub fn size_by(&mut self, step: i16, cx: &mut Context<Self>) {
        let size = (self.library.settings.size as i16 + step).clamp(15, 31) as u16;
        if size != self.library.settings.size {
            self.library.settings.size = size;
            self.list.remeasure();
            self.save_now(cx);
        }
    }

    pub fn picture(&self, book: &BookInfo, path: &str) -> Option<&ImageState> {
        self.pictures.get(&book.id)?.get(path)
    }

    pub fn request_picture(&mut self, book: BookInfo, path: String, cx: &mut Context<Self>) {
        if self.picture(&book, &path).is_some() {
            return;
        }
        let Some(archive) = self.store.as_ref().and_then(|store| store.path_for(&book)) else {
            return;
        };
        self.pictures
            .entry(book.id.clone())
            .or_default()
            .insert(path.clone(), ImageState::Loading);
        self.image_fifo.push_back((book.id.clone(), path.clone()));
        let cache_limit = if book.format == Format::Pdf { 8 } else { 48 };
        if self.image_fifo.len() > cache_limit
            && let Some(index) = self.image_fifo.iter().position(|(id, path)| {
                self.pictures
                    .get(id)
                    .and_then(|images| images.get(path))
                    .is_some_and(|state| !matches!(state, ImageState::Loading))
            })
            && let Some((id, path)) = self.image_fifo.remove(index)
            && let Some(images) = self.pictures.get_mut(&id)
        {
            images.remove(&path);
            if images.is_empty() {
                self.pictures.remove(&id);
            }
        }
        cx.spawn(async move |weak, cx| {
            let name = path.clone();
            let format = book.format;
            let result = cx
                .background_spawn(async move {
                    if format == Format::Pdf {
                        pdf::render(&archive, &name)
                    } else {
                        read_picture(&archive, &name)
                    }
                })
                .await;
            let _ = weak.update(cx, |reader, cx| {
                let Some(state) = reader.pictures.get_mut(&book.id).and_then(|images| images.get_mut(&path)) else { return };
                *state = result.map_or(ImageState::Failed, ImageState::Ready);
                if reader.book.info.id == book.id && reader.screen == Screen::Reading {
                    for (row, block) in reader.data().blocks.iter().enumerate() {
                        if matches!(&block.kind, crate::book::BlockKind::Image { path: asset, .. } if asset == &path) {
                            reader.list.remeasure_items((row + 1)..(row + 2));
                        }
                    }
                }
                cx.notify();
            });
        }).detach();
    }

    pub fn page_scroll(&mut self, forward: bool, window: &Window, cx: &mut Context<Self>) {
        if self.busy || self.editor.is_some() {
            return;
        }
        if self.is_paged() {
            self.turn(forward, cx);
            return;
        }
        let distance = (self.list.viewport_bounds().size.height - px(70.)).max(px(120.));
        self.list
            .scroll_by(if forward { distance } else { -distance });
        self.scrolled(window, cx);
    }
}

fn theme(paper: Paper) -> ThemeMode {
    if paper == Paper::Night {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}
