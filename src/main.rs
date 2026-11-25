#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod book;
#[cfg(any(target_os = "windows", test))]
mod controller;
mod demo;
mod epub;
mod epub_style;

mod library;
mod pictures;
mod reader;
mod shelf_folders;
mod shelf_view;

mod pagination;
mod plugin_view;
mod plugins;
mod reading_view;
mod style;
#[cfg(test)]
mod tests;
mod ui;

use std::path::PathBuf;

use gpui_kit::*;

actions!(moye, [Quit]);

fn main() {
    let _ = tracing_subscriber::fmt().try_init();
    let mut handheld = cfg!(target_os = "windows");
    let mut files = Vec::new();
    let mut options = true;
    for arg in std::env::args_os().skip(1) {
        if options && arg == "--" {
            options = false;
        } else if options && arg == "--handheld" {
            handheld = true;
        } else if options && arg == "--desktop" {
            handheld = false;
        } else if !options || !arg.to_string_lossy().starts_with("-psn_") {
            files.push(PathBuf::from(arg));
        }
    }
    application().with_assets(assets::AllAssets).run(move |cx| {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        load_bundled_fonts(cx);
        init(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-q"
            } else {
                "ctrl-q"
            },
            Quit,
            None,
        )]);
        cx.set_menus(vec![Menu {
            name: "墨页".into(),
            disabled: false,
            items: vec![MenuItem::action("退出墨页", Quit)],
        }]);
        let (_preferred, minimum) = if handheld {
            (size(px(1100.), px(760.)), size(px(640.), px(400.)))
        } else {
            (size(px(1280.), px(825.)), size(px(930.), px(595.)))
        };
        // let bounds = Bounds::centered(None, preferred, cx);
        let window = WindowOptions {
            // window_bounds: Some(match (handheld, std::env::consts::OS) {
            //     (true, "linux") => WindowBounds::Fullscreen(bounds),
            //     (true, "windows") => WindowBounds::Maximized(bounds),
            //     _ => WindowBounds::Windowed(bounds),
            // }),
            window_min_size: Some(minimum),
            app_owns_titlebar_drag: cfg!(target_os = "macos"),
            titlebar: Some(TitlebarOptions {
                title: Some("墨页".into()),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: cfg!(target_os = "macos")
                    .then(|| ui::traffic_light_position(reader::Screen::Library)),
            }),
            app_id: Some("app.moye.reader".to_owned()),
            ..Default::default()
        };
        let (_window, _reader) = open_window(window, cx, |window, cx| {
            cx.new(|cx| reader::Reader::new(files, handheld, window, cx))
        })
        .expect("无法打开墨页窗口");
        #[cfg(target_os = "windows")]
        if handheld {
            controller::listen(_window, cx);
        }
        cx.activate(true);
    });
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn load_bundled_fonts(cx: &mut App) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let folder = executable.with_file_name("fonts");
    let fonts = ["NotoSansCJKsc-Regular.otf", "NotoSerifCJKsc-Regular.otf"]
        .into_iter()
        .filter_map(|name| std::fs::read(folder.join(name)).ok())
        .map(std::borrow::Cow::Owned)
        .collect::<Vec<_>>();
    if !fonts.is_empty()
        && let Err(error) = cx.text_system().add_fonts(fonts)
    {
        eprintln!("无法载入随附字体：{error}");
    }
}
