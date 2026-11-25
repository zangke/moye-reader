use std::{fs, path::PathBuf, time::Duration};

use anyhow::Result;
use gpui_kit::{prelude::FluentBuilder as _, *};
use moye_plugin_api::{Call, DirectorySettings, ItemKind, Listing, MAX_PAGE_SIZE};
use serde_json::Value;
use tempfile::TempDir;

use crate::{
    plugins::{self, Catalog, Plugin, Running},
    reader::{Reader, Screen},
    style::{CLOSE, FILE, LEFT, Palette, RIGHT, icon, label, surface},
    ui::button_style,
};

const POLL_INTERVAL: Duration = Duration::from_millis(80);
pub(crate) const PLUGIN_ICON: &str = "icons/puzzle.svg";

#[derive(Default)]
pub(crate) struct PluginPage {
    catalog: Catalog,
    task: Option<Task<()>>,
    error: Option<String>,
    settings: Option<PluginSettings>,
}

struct PluginSettings {
    plugin: Plugin,
    directory: Option<PathBuf>,
    task: Option<Task<()>>,
    message: Option<String>,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct Location {
    folders: Vec<(String, String)>,
    cursors: Vec<String>,
}

struct Browser {
    plugin: Plugin,
    location: Location,
    listing: Option<Listing>,
}

pub(crate) struct SourcesPanel {
    focus: FocusHandle,
    plugins: Vec<Plugin>,
    browser: Option<Browser>,
    task: Option<Task<()>>,
    progress: Option<String>,
    error: Option<String>,
}

enum Operation {
    List(Location),
    Fetch(String),
    Configure,
    Disconnect,
}

impl Operation {
    fn request(&self) -> Call {
        match self {
            Self::List(location) => Call::List {
                folder: location.folders.last().map(|(id, _)| id.clone()),
                cursor: location.cursors.last().cloned(),
                query: None,
                limit: MAX_PAGE_SIZE,
            },
            Self::Fetch(id) => Call::Fetch {
                ids: vec![id.clone()],
            },
            Self::Configure => Call::Configure,
            Self::Disconnect => Call::Disconnect,
        }
    }
}

enum Reply {
    Page(Location, Listing),
    Books(Vec<PathBuf>, TempDir),
    ConnectionChanged,
}

async fn invoke_plugin(
    plugin: Plugin,
    call: Call,
    cx: &mut AsyncWindowContext,
    mut on_progress: impl FnMut(String, &mut AsyncWindowContext),
) -> Result<(Running, Value)> {
    let mut running = cx
        .background_spawn(async move { Running::start(plugin, call) })
        .await?;
    loop {
        let (next, status, progress) = cx
            .background_spawn(async move {
                let status = running.poll();
                let progress = running.progress();
                (running, status, progress)
            })
            .await;
        running = next;
        if let Some(value) = status? {
            return Ok((running, value));
        }
        if let Some(progress) = progress {
            on_progress(progress, cx);
        }
        cx.background_executor().timer(POLL_INTERVAL).await;
    }
}

impl Reader {
    pub(crate) fn open_plugins(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Plugins {
            return;
        }
        self.set_screen(Screen::Plugins, window);
        self.focus.focus(window, cx);
        self.plugin_page = Some(PluginPage::default());
        self.refresh_plugins(window, cx);
    }

    fn refresh_plugins(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(page) = self.plugin_page.as_mut().filter(|page| page.task.is_none()) else {
            return;
        };
        let Some(root) = self.store.as_ref().map(|store| store.root().to_owned()) else {
            page.error = Some("本地书库不可用，无法载入插件。".to_owned());
            cx.notify();
            return;
        };
        page.error = None;
        page.task = Some(cx.spawn_in(window, async move |weak, cx| {
            let result = cx
                .background_spawn(async move { plugins::discover(&root) })
                .await;
            let _ = weak.update(cx, |reader, cx| {
                let Some(page) = reader.plugin_page.as_mut() else {
                    return;
                };
                page.task = None;
                match result {
                    Ok(catalog) => page.catalog = catalog,
                    Err(error) => page.error = Some(format!("无法读取插件目录：{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn toggle_plugin(&mut self, id: &str, cx: &mut Context<Self>) {
        let disabled = &mut self.library.settings.disabled_plugins;
        if !disabled.remove(id) {
            disabled.insert(id.to_owned());
        }
        self.save_now(cx);
    }

    fn open_plugin_directory(&mut self, window: &Window, cx: &mut Context<Self>) {
        if let Some(store) = &self.store {
            let dir = store.root().join("plugins");
            match fs::create_dir_all(&dir) {
                Ok(()) => cx.open_with_system(&dir),
                Err(error) => self.tell(format!("无法打开插件文件夹：{error}"), true, window, cx),
            }
        }
    }

    fn open_plugin_settings(&mut self, plugin: Plugin, window: &Window, cx: &mut Context<Self>) {
        let Some(page) = self.plugin_page.as_mut() else {
            return;
        };
        if !plugin.has_settings() || self.library.settings.disabled_plugins.contains(&plugin.id) {
            return;
        }
        page.settings = Some(PluginSettings {
            plugin,
            directory: None,
            task: None,
            message: None,
            error: None,
        });
        if page
            .settings
            .as_ref()
            .is_some_and(|settings| settings.plugin.has_directory_settings())
        {
            self.run_plugin_settings(Call::DirectorySettings, window, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn close_plugin_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .plugin_page
            .as_mut()
            .and_then(|page| page.settings.take())
            .is_none()
        {
            return false;
        }
        self.focus.focus(window, cx);
        cx.notify();
        true
    }

    fn choose_plugin_directory(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(settings) = self
            .plugin_page
            .as_mut()
            .and_then(|page| page.settings.as_mut())
            .filter(|settings| settings.task.is_none())
        else {
            return;
        };
        settings.error = None;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("选择书籍文件夹".into()),
        });
        settings.task = Some(cx.spawn_in(window, async move |weak, cx| {
            let selection = paths.await;
            let _ = weak.update_in(cx, |reader, window, cx| {
                let Some(settings) = reader
                    .plugin_page
                    .as_mut()
                    .and_then(|page| page.settings.as_mut())
                else {
                    return;
                };
                settings.task = None;
                match selection {
                    Ok(Ok(Some(paths))) => {
                        if let Some(directory) = paths.into_iter().next() {
                            reader.run_plugin_settings(
                                Call::SetDirectory { directory },
                                window,
                                cx,
                            );
                        }
                    }
                    Ok(Err(error)) => settings.error = Some(format!("无法打开目录选择器：{error}")),
                    _ => {}
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn run_plugin_settings(&mut self, call: Call, window: &Window, cx: &mut Context<Self>) {
        let Some(settings) = self
            .plugin_page
            .as_mut()
            .and_then(|page| page.settings.as_mut())
            .filter(|settings| {
                settings.task.is_none() && settings.plugin.supports(call.capability())
            })
        else {
            return;
        };
        let (pending, success, returns_directory) = match call {
            Call::DirectorySettings => ("正在读取设置…", None, true),
            Call::SetDirectory { .. } => ("正在保存目录…", Some("已保存书籍目录"), true),
            Call::Configure => ("正在连接账户…", Some("账户连接已完成"), false),
            Call::Disconnect => ("正在断开连接…", Some("账户已断开"), false),
            _ => return,
        };
        settings.message = Some(pending.to_owned());
        settings.error = None;
        let plugin = settings.plugin.clone();
        settings.task = Some(cx.spawn_in(window, async move |weak, cx| {
            let result = invoke_plugin(plugin, call, cx, |progress, cx| {
                let _ = weak.update(cx, |reader, cx| {
                    if let Some(settings) = reader
                        .plugin_page
                        .as_mut()
                        .and_then(|page| page.settings.as_mut())
                    {
                        settings.message = Some(progress);
                        cx.notify();
                    }
                });
            })
            .await
            .and_then(|(_, value)| {
                if returns_directory {
                    Ok(Some(serde_json::from_value::<DirectorySettings>(value)?))
                } else {
                    Ok(None)
                }
            });
            let _ = weak.update(cx, |reader, cx| {
                let Some(settings) = reader
                    .plugin_page
                    .as_mut()
                    .and_then(|page| page.settings.as_mut())
                else {
                    return;
                };
                settings.task = None;
                settings.message = None;
                match result {
                    Ok(directory) => {
                        if let Some(directory) = directory {
                            settings.directory = directory.directory;
                        }
                        settings.message = success.map(str::to_owned);
                    }
                    Err(error) => settings.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn plugin_settings_view(
        &self,
        settings: &PluginSettings,
        p: Palette,
        cx: &Context<Self>,
    ) -> AnyElement {
        div()
            .id("plugin-settings")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    // .max_w(px(1160.))//TODO
                    .px(px(if self.handheld { 24. } else { 36. }))
                    .py(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(15.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                source_button("plugin-settings-back", p)
                                    .child(icon(LEFT, 16., p.soft))
                                    .child("插件")
                                    .on_click(cx.listener(|reader, _, window, cx| {
                                        reader.close_plugin_settings(window, cx);
                                    })),
                            )
                            .child(
                                label(settings.plugin.name.clone(), 24., p.ink)
                                    .font_weight(FontWeight::SEMIBOLD),
                            ),
                    )
                    .when(settings.plugin.has_directory_settings(), |body| {
                        body.child(
                            surface(p)
                                .p(px(17.))
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.))
                                        .child(
                                            label("书籍目录", 15., p.ink)
                                                .font_weight(FontWeight::SEMIBOLD),
                                        )
                                        .child(
                                            label(
                                                settings.directory.as_ref().map_or_else(
                                                    || "请选择存放 TXT 或 EPUB 的文件夹".to_owned(),
                                                    |directory| directory.display().to_string(),
                                                ),
                                                13.,
                                                p.dim,
                                            )
                                            .line_clamp(3),
                                        ),
                                )
                                .child(
                                    source_button("plugin-select-directory", p)
                                        .child(icon("icons/folder-open.svg", 16., p.soft))
                                        .child(if settings.directory.is_some() {
                                            "更换文件夹"
                                        } else {
                                            "选择文件夹"
                                        })
                                        .on_click(cx.listener(|reader, _, window, cx| {
                                            reader.choose_plugin_directory(window, cx)
                                        })),
                                ),
                        )
                    })
                    .when(
                        settings.plugin.supports("source.configure")
                            || settings.plugin.supports("source.disconnect"),
                        |body| {
                            body.child(
                                surface(p)
                                    .p(px(17.))
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(
                                        label("账户", 15., p.ink)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .flex_1(),
                                    )
                                    .when(settings.plugin.supports("source.configure"), |row| {
                                        row.child(
                                            source_button("plugin-connect", p)
                                                .child("连接账户")
                                                .on_click(cx.listener(|reader, _, window, cx| {
                                                    reader.run_plugin_settings(
                                                        Call::Configure,
                                                        window,
                                                        cx,
                                                    )
                                                })),
                                        )
                                    })
                                    .when(settings.plugin.supports("source.disconnect"), |row| {
                                        row.child(
                                            source_button("plugin-disconnect", p)
                                                .child("断开连接")
                                                .on_click(cx.listener(|reader, _, window, cx| {
                                                    reader.run_plugin_settings(
                                                        Call::Disconnect,
                                                        window,
                                                        cx,
                                                    )
                                                })),
                                        )
                                    }),
                            )
                        },
                    )
                    .when_some(settings.message.as_ref(), |body, message| {
                        body.child(label(message.clone(), 13., p.green))
                    })
                    .when_some(settings.error.as_ref(), |body, error| {
                        body.child(label(error.clone(), 13., p.danger))
                    }),
            )
            .into_any_element()
    }

    pub(crate) fn plugins_page(&self, p: Palette, cx: &Context<Self>) -> AnyElement {
        let page = self.plugin_page.as_ref();
        if let Some(settings) = page.and_then(|page| page.settings.as_ref()) {
            return self.plugin_settings_view(settings, p, cx);
        }
        let loading = page.is_some_and(|page| page.task.is_some());
        let installed = page.map_or(&[][..], |page| page.catalog.plugins.as_slice());
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(
                label("插件", 29., p.ink)
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex_1(),
            )
            .child(
                source_button("plugins-refresh", p)
                    .child(icon("icons/refresh-cw.svg", 14., p.soft))
                    .child(if loading { "正在刷新…" } else { "刷新" })
                    .on_click(
                        cx.listener(|reader, _, window, cx| reader.refresh_plugins(window, cx)),
                    ),
            )
            .child(
                button_style("plugins-folder", p.white, p.green)
                    .h(px(36.))
                    .bg(rgb(p.button))
                    .child(icon("icons/folder-open.svg", 16., p.white))
                    .child("打开插件文件夹")
                    .on_click(cx.listener(|reader, _, window, cx| {
                        reader.open_plugin_directory(window, cx)
                    })),
            );
        div()
            .id("plugins-page")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                div()
                    .mx_auto()
                    .w_full()
                    // .max_w(px(1160.))//TODO
                    .px(px(if self.handheld { 24. } else { 36. }))
                    .pt(px(30.))
                    .pb(px(30.))
                    .flex()
                    .flex_col()
                    .gap(px(15.))
                    .child(header)
                    .when(
                        installed.is_empty()
                            && !loading
                            && page.is_some_and(|page| {
                                page.catalog.errors.is_empty() && page.error.is_none()
                            }),
                        |body| {
                            body.child(
                                surface(p)
                                    .px(px(20.))
                                    .py(px(17.))
                                    .flex()
                                    .flex_col()
                                    .gap(px(7.))
                                    .child(
                                        label("尚未安装插件", 15., p.ink)
                                            .font_weight(FontWeight::SEMIBOLD),
                                    )
                                    .child(label(
                                        "把插件目录放进插件文件夹，再点刷新。",
                                        13.,
                                        p.dim,
                                    )),
                            )
                        },
                    )
                    .children(
                        installed
                            .iter()
                            .enumerate()
                            .map(|(index, plugin)| self.plugin_row(index, plugin, p, cx)),
                    )
                    .when_some(page.and_then(|page| page.error.as_ref()), |body, error| {
                        body.child(label(error.clone(), 13., p.danger))
                    })
                    .when_some(page, |body, page| {
                        body.children(page.catalog.errors.iter().map(|error| {
                            surface(p).px(px(15.)).py(px(10.)).child(label(
                                error.clone(),
                                12.,
                                p.danger,
                            ))
                        }))
                    }),
            )
            .into_any_element()
    }

    fn plugin_row(&self, index: usize, plugin: &Plugin, p: Palette, cx: &Context<Self>) -> Div {
        let id = plugin.id.clone();
        let enabled = !self.library.settings.disabled_plugins.contains(&id);
        let directory = plugin.directory().to_owned();
        let purpose = if plugin.is_book_source() {
            "书源插件"
        } else {
            "扩展插件"
        };
        surface(p)
            .px(px(16.))
            .py(px(13.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .size(px(40.))
                    .rounded(px(8.))
                    .flex_shrink_0()
                    .bg(rgb(p.green_bg))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(PLUGIN_ICON, 19., p.green)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        label(plugin.name.clone(), 15., p.ink)
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .text_ellipsis(),
                    )
                    .child(
                        label(format!("{purpose} · {id}"), 12., p.dim)
                            .whitespace_nowrap()
                            .text_ellipsis(),
                    ),
            )
            .child(
                label(
                    if enabled { "已启用" } else { "已停用" },
                    12.,
                    if enabled { p.green } else { p.dim },
                )
                .flex_shrink_0()
                .rounded(px(5.))
                .px(px(8.))
                .py(px(4.))
                .bg(rgb(if enabled { p.green_bg } else { p.rail })),
            )
            .child(div().when(enabled && plugin.has_settings(), |actions| {
                let plugin = plugin.clone();
                actions.child(
                    source_button(("plugin-settings", index), p)
                        .child(icon("icons/settings.svg", 15., p.soft))
                        .child("设置")
                        .on_click(cx.listener(move |reader, _, window, cx| {
                            reader.open_plugin_settings(plugin.clone(), window, cx)
                        })),
                )
            }))
            .child(
                source_button(("plugin-open-folder", index), p)
                    .aria_label("打开此插件的目录")
                    .px(px(8.))
                    .child(icon("icons/folder-open.svg", 16., p.soft))
                    .on_click(move |_, _, cx| cx.open_with_system(&directory)),
            )
            .child(
                source_button(("plugin-toggle", index), p)
                    .child(if enabled { "停用" } else { "启用" })
                    .on_click(cx.listener(move |reader, _, _, cx| reader.toggle_plugin(&id, cx))),
            )
    }

    pub(crate) fn open_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.sources.is_some() || self.screen != Screen::Library {
            return;
        }
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.sources = Some(SourcesPanel {
            focus,
            plugins: Vec::new(),
            browser: None,
            task: None,
            progress: None,
            error: None,
        });
        self.reload_sources(window, cx);
    }

    fn reload_sources(&mut self, window: &Window, cx: &mut Context<Self>) {
        let Some(panel) = self.sources.as_mut().filter(|panel| panel.task.is_none()) else {
            return;
        };
        let Some(root) = self.store.as_ref().map(|store| store.root().to_owned()) else {
            panel.error = Some("本地书库不可用，无法载入插件。".to_owned());
            cx.notify();
            return;
        };
        panel.error = None;
        panel.progress = Some("正在查找书源插件…".to_owned());
        panel.task = Some(cx.spawn_in(window, async move |weak, cx| {
            let result = cx
                .background_spawn(async move { plugins::discover(&root) })
                .await;
            let _ = weak.update(cx, |reader, cx| {
                if let Some(panel) = reader.sources.as_mut() {
                    panel.task = None;
                    panel.progress = None;
                    match result {
                        Ok(mut catalog) => {
                            catalog.plugins.retain(|plugin| {
                                plugin.is_book_source()
                                    && !reader
                                        .library
                                        .settings
                                        .disabled_plugins
                                        .contains(&plugin.id)
                            });
                            panel.plugins = catalog.plugins;
                        }
                        Err(error) => panel.error = Some(format!("无法查找书源插件：{error:#}")),
                    }
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    pub(crate) fn close_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sources = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn sources_key(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(panel) = self.sources.as_ref() else {
            return false;
        };
        if key == "escape" {
            self.close_sources(window, cx);
            return true;
        }
        let forward = match key {
            "up" | "left" => false,
            "down" | "right" => true,
            _ => return false,
        };
        if !panel.focus.contains_focused(window, cx) {
            panel.focus.focus(window, cx);
        }
        let previous = window.focused(cx);
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        if !panel.focus.contains_focused(window, cx)
            && let Some(previous) = previous
        {
            previous.focus(window, cx);
        }
        true
    }

    fn choose_source(&mut self, plugin: Plugin, window: &Window, cx: &mut Context<Self>) {
        let Some(panel) = self.sources.as_mut().filter(|panel| panel.task.is_none()) else {
            return;
        };
        panel.browser = Some(Browser {
            plugin,
            location: Location::default(),
            listing: None,
        });
        self.run_source(Operation::List(Location::default()), window, cx);
    }

    fn run_source(&mut self, operation: Operation, window: &Window, cx: &mut Context<Self>) {
        let Some(panel) = self.sources.as_mut().filter(|panel| panel.task.is_none()) else {
            return;
        };
        let Some(plugin) = panel.browser.as_ref().map(|browser| browser.plugin.clone()) else {
            return;
        };
        panel.error = None;
        panel.progress = Some(
            match operation {
                Operation::List(_) => "正在读取目录…",
                Operation::Fetch(_) => "正在下载书籍…",
                Operation::Configure => "正在连接书源…",
                Operation::Disconnect => "正在断开连接…",
            }
            .to_owned(),
        );
        panel.task = Some(cx.spawn_in(window, async move |weak, cx| {
            let request = operation.request();
            let result: Result<Reply> = async {
                let (mut running, value) = invoke_plugin(plugin, request, cx, |progress, cx| {
                    let _ = weak.update(cx, |reader, cx| {
                        if let Some(panel) = reader.sources.as_mut() {
                            panel.progress = Some(progress);
                            cx.notify();
                        }
                    });
                })
                .await?;
                cx.background_spawn(async move {
                    match operation {
                        Operation::List(location) => {
                            Ok(Reply::Page(location, plugins::listing(value)?))
                        }
                        Operation::Fetch(_) => {
                            let (paths, scratch) = running.downloads(value, 1)?;
                            Ok(Reply::Books(paths, scratch))
                        }
                        Operation::Configure | Operation::Disconnect => {
                            Ok(Reply::ConnectionChanged)
                        }
                    }
                })
                .await
            }
            .await;
            let _ = weak.update_in(cx, |reader, window, cx| {
                let Some(panel) = reader.sources.as_mut() else {
                    return;
                };
                panel.task = None;
                panel.progress = None;
                match result {
                    Ok(Reply::Page(location, listing)) => {
                        if let Some(browser) = panel.browser.as_mut() {
                            browser.location = location;
                            browser.listing = Some(listing);
                        }
                    }
                    Ok(Reply::Books(paths, scratch)) => {
                        reader.import_files(paths, Some(scratch), window, cx)
                    }
                    Ok(Reply::ConnectionChanged) => {
                        reader.run_source(Operation::List(Location::default()), window, cx)
                    }
                    Err(error) => panel.error = Some(format!("{error:#}")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(crate) fn sources_view(
        &self,
        panel: &SourcesPanel,
        window: &Window,
        p: Palette,
        cx: &Context<Self>,
    ) -> Div {
        let viewport = window.viewport_size();
        let waiting = panel.task.is_some();
        let row_count = panel.browser.as_ref().map_or_else(
            || panel.plugins.len(),
            |browser| {
                browser
                    .listing
                    .as_ref()
                    .map_or(0, |listing| listing.items.len())
            },
        );
        let height = px(148. + 45. * row_count.min(10) as f32)
            .clamp(px(260.), px(560.))
            .min(viewport.height - px(32.));
        let title = panel
            .browser
            .as_ref()
            .map_or("书籍来源", |browser| browser.plugin.name.as_str());
        let header = div()
            .flex()
            .items_center()
            .gap(px(7.))
            .when(panel.browser.is_some(), |header| {
                header.child(
                    source_button("sources-back", p)
                        .aria_label("返回书籍来源")
                        .child(icon(LEFT, 16., p.soft))
                        .on_click(cx.listener(|reader, _, window, cx| {
                            if let Some(panel) = reader.sources.as_mut() {
                                panel.task = None;
                                panel.browser = None;
                                panel.progress = None;
                                panel.error = None;
                                panel.focus.focus(window, cx);
                                cx.notify();
                            }
                        })),
                )
            })
            .child(
                label(title.to_owned(), 16., p.ink)
                    .font_weight(FontWeight::SEMIBOLD)
                    .min_w_0()
                    .flex_1()
                    .text_ellipsis(),
            )
            .child(
                source_button("sources-close", p)
                    .aria_label(if waiting {
                        "取消并关闭"
                    } else {
                        "关闭书籍来源"
                    })
                    .child(icon(CLOSE, 16., p.soft))
                    .on_click(
                        cx.listener(|reader, _, window, cx| reader.close_sources(window, cx)),
                    ),
            );
        let body = if let Some(browser) = &panel.browser {
            self.source_browser(browser, waiting, p, cx)
        } else {
            self.source_catalog(&panel.plugins, waiting, p, cx)
        };
        div()
            .absolute()
            .inset_0()
            .size_full()
            .bg(rgba(0x121b1555))
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .child(
                surface(p)
                    .w((viewport.width - px(32.)).min(px(580.)))
                    .h(height)
                    .p(px(16.))
                    .shadow_lg()
                    .occlude()
                    .tab_index(0)
                    .tab_group()
                    .track_focus(&panel.focus)
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(header)
                    .when_some(panel.progress.as_ref(), |card, progress| {
                        card.child(label(progress.clone(), 12., p.green).flex_shrink_0())
                    })
                    .when_some(panel.error.as_ref(), |card, error| {
                        card.child(
                            label(error.clone(), 12., p.danger)
                                .flex_shrink_0()
                                .line_clamp(3),
                        )
                    })
                    .child(body),
            )
    }

    fn source_catalog(
        &self,
        plugins: &[Plugin],
        waiting: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(8.))
            .child(
                div()
                    .id("source-catalog")
                    .overflow_y_scroll()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .when(plugins.is_empty() && !waiting, |list| {
                        list.child(label("没有启用的书源插件。", 13., p.dim))
                    })
                    .children(plugins.iter().enumerate().map(|(index, plugin)| {
                        let plugin = plugin.clone();
                        source_button(("source-plugin", index), p)
                            .h(px(43.))
                            .justify_between()
                            .child(plugin.name.clone())
                            .child(icon(RIGHT, 15., p.dim))
                            .on_click(cx.listener(move |reader, _, window, cx| {
                                reader.choose_source(plugin.clone(), window, cx)
                            }))
                    })),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        source_button("source-manage-plugins", p)
                            .child("管理插件")
                            .on_click(
                                cx.listener(|reader, _, window, cx| {
                                    reader.open_plugins(window, cx)
                                }),
                            ),
                    )
                    .child(source_button("source-rescan", p).child("刷新").on_click(
                        cx.listener(|reader, _, window, cx| reader.reload_sources(window, cx)),
                    )),
            )
    }

    fn source_browser(
        &self,
        browser: &Browser,
        waiting: bool,
        p: Palette,
        cx: &Context<Self>,
    ) -> Div {
        let location = &browser.location;
        let breadcrumb = location
            .folders
            .iter()
            .map(|(_, name)| name.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let list = browser.listing.as_ref();
        let mut rows = div()
            .id("source-files")
            .overflow_y_scroll()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(3.));
        if let Some(list) = list {
            rows = rows
                .when(list.needs_connection, |rows| {
                    rows.child(label("连接账户后即可浏览书籍。", 13., p.dim))
                })
                .when(
                    list.items.is_empty() && !list.needs_connection && !waiting,
                    |rows| rows.child(label("这里没有 TXT 或 EPUB。", 13., p.dim)),
                )
                .children(list.items.iter().enumerate().map(|(index, item)| {
                    let id = item.id.clone();
                    let name = item.name.clone();
                    let folder = item.kind == ItemKind::Folder;
                    let mut target = location.clone();
                    if folder {
                        target.folders.push((id.clone(), name.clone()));
                        target.cursors.clear();
                    }
                    source_button(("source-item", index), p)
                        .h(px(42.))
                        .justify_start()
                        .min_w_0()
                        .child(icon(
                            if folder { "icons/folder.svg" } else { FILE },
                            16.,
                            p.green,
                        ))
                        .child(
                            label(name, 13., p.ink)
                                .min_w_0()
                                .flex_1()
                                .whitespace_nowrap()
                                .text_ellipsis(),
                        )
                        .child(label(if folder { "打开" } else { "导入" }, 12., p.dim))
                        .on_click(cx.listener(move |reader, _, window, cx| {
                            reader.run_source(
                                if folder {
                                    Operation::List(target.clone())
                                } else {
                                    Operation::Fetch(id.clone())
                                },
                                window,
                                cx,
                            )
                        }))
                }));
        }
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .when(!location.folders.is_empty(), |bar| {
                        let mut parent = location.clone();
                        parent.folders.pop();
                        parent.cursors.clear();
                        bar.child(source_button("source-parent", p).child("上一级").on_click(
                            cx.listener(move |reader, _, window, cx| {
                                reader.run_source(Operation::List(parent.clone()), window, cx)
                            }),
                        ))
                    })
                    .child(
                        label(
                            if breadcrumb.is_empty() {
                                "根目录".to_owned()
                            } else {
                                breadcrumb
                            },
                            12.,
                            p.dim,
                        )
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis(),
                    )
                    .when(browser.plugin.supports("source.configure"), |bar| {
                        bar.child(
                            source_button("source-configure", p)
                                .child("连接设置")
                                .on_click(cx.listener(|reader, _, window, cx| {
                                    reader.run_source(Operation::Configure, window, cx)
                                })),
                        )
                    })
                    .when(browser.plugin.supports("source.disconnect"), |bar| {
                        bar.child(
                            source_button("source-disconnect", p)
                                .child("断开")
                                .on_click(cx.listener(|reader, _, window, cx| {
                                    reader.run_source(Operation::Disconnect, window, cx)
                                })),
                        )
                    }),
            )
            .child(rows)
            .child(
                div()
                    .flex()
                    .gap(px(7.))
                    .justify_end()
                    .when(!location.cursors.is_empty(), |bar| {
                        let mut previous = location.clone();
                        previous.cursors.pop();
                        bar.child(
                            source_button("source-page-back", p)
                                .child("上一批")
                                .on_click(cx.listener(move |reader, _, window, cx| {
                                    reader.run_source(Operation::List(previous.clone()), window, cx)
                                })),
                        )
                    })
                    .when_some(
                        list.and_then(|list| list.next_cursor.clone()),
                        |bar, cursor| {
                            let mut next = location.clone();
                            next.cursors.push(cursor);
                            bar.child(
                                source_button("source-page-forward", p)
                                    .child("下一批")
                                    .on_click(cx.listener(move |reader, _, window, cx| {
                                        reader.run_source(Operation::List(next.clone()), window, cx)
                                    })),
                            )
                        },
                    )
                    .child({
                        let location = location.clone();
                        source_button("source-reload", p)
                            .child("刷新")
                            .on_click(cx.listener(move |reader, _, window, cx| {
                                reader.run_source(Operation::List(location.clone()), window, cx)
                            }))
                    }),
            )
    }
}

fn source_button(id: impl Into<ElementId>, p: Palette) -> Stateful<Div> {
    button_style(id, p.soft, p.hover).h(px(36.))
}
