use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use moye_plugin_api::{
    Call, DirectorySettings, Download, Event, Item, ItemKind, Listing, MAX_BOOK_BYTES,
    MAX_PAGE_SIZE, PROTOCOL_VERSION, Request,
};
use serde_json::Value;

fn main() -> Result<()> {
    let mut stdout = io::stdout().lock();
    match serve(&mut stdout) {
        Ok(value) => send(&mut stdout, Event::Result { value }),
        Err(error) => send(
            &mut stdout,
            Event::Error {
                message: format!("{error:#}"),
            },
        ),
    }
}

fn serve(stdout: &mut impl Write) -> Result<Value> {
    let request: Request = serde_json::from_reader(io::stdin().lock())?;
    ensure!(request.protocol == PROTOCOL_VERSION, "插件协议版本不匹配");
    match request.call {
        Call::DirectorySettings => Ok(serde_json::to_value(directory_settings(
            &request.data_dir,
        )?)?),
        Call::SetDirectory { directory } => {
            ensure!(
                directory.is_absolute() && directory.is_dir(),
                "所选书籍目录不可用"
            );
            let settings = DirectorySettings {
                directory: Some(directory),
            };
            let mut file = tempfile::NamedTempFile::new_in(&request.data_dir)?;
            serde_json::to_writer(&mut file, &settings)?;
            file.as_file().sync_all()?;
            file.persist(request.data_dir.join("directory.json"))?;
            Ok(serde_json::to_value(settings)?)
        }
        Call::List {
            folder,
            cursor,
            query,
            limit,
        } => {
            let root = source_root(&request.data_dir)?;
            let path = folder
                .as_deref()
                .map_or(Ok(root.clone()), |id| resolve(&root, id))?;
            let start = cursor.map_or(Ok(0), |cursor| cursor.parse::<usize>())?;
            let query = query.map(|query| query.to_lowercase());
            let mut files = Vec::new();
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let Some(name) = entry.file_name().into_string().ok() else {
                    continue;
                };
                let entry_type = entry.file_type()?;
                let path = entry.path();
                let kind = if entry_type.is_dir() {
                    ItemKind::Folder
                } else if entry_type.is_file() && book_file(&path) {
                    ItemKind::Book
                } else {
                    continue;
                };
                if name.chars().any(char::is_control)
                    || name.contains([':', '\\'])
                    || query
                        .as_ref()
                        .is_some_and(|query| !name.to_lowercase().contains(query))
                {
                    continue;
                }
                files.push(Item {
                    id: folder
                        .as_ref()
                        .map_or_else(|| name.clone(), |folder| format!("{folder}/{name}")),
                    name,
                    kind,
                    size: if kind == ItemKind::Book {
                        Some(entry.metadata()?.len())
                    } else {
                        None
                    },
                });
            }
            files.sort_by(|a, b| {
                (a.kind == ItemKind::Book, &a.name).cmp(&(b.kind == ItemKind::Book, &b.name))
            });
            let end = start.saturating_add(limit.clamp(1, MAX_PAGE_SIZE));
            Ok(serde_json::to_value(Listing {
                next_cursor: (end < files.len()).then(|| end.to_string()),
                items: files.into_iter().skip(start).take(end - start).collect(),
                needs_connection: false,
            })?)
        }
        Call::Fetch { ids } => {
            let root = source_root(&request.data_dir)?;
            ensure!(
                !ids.is_empty() && ids.len() <= MAX_PAGE_SIZE,
                "请求的书籍数量不合规"
            );
            let total = ids.len() as u64;
            let mut files = Vec::new();
            for (index, id) in ids.into_iter().enumerate() {
                let path = resolve(&root, &id)?;
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .context("文件名无效")?;
                ensure!(
                    path.is_file() && book_file(&path),
                    "只能导入 TXT、EPUB 或 PDF 文件"
                );
                let source = File::open(&path)?;
                ensure!(
                    source.metadata()?.len() <= MAX_BOOK_BYTES,
                    "书籍超过 256 MB 导入上限"
                );
                send(
                    stdout,
                    Event::Progress {
                        message: format!("正在读取 {name}"),
                        completed: Some(index as u64),
                        total: Some(total),
                    },
                )?;
                let mut output = File::create_new(request.output_dir.join(name))?;
                let bytes = io::copy(&mut source.take(MAX_BOOK_BYTES + 1), &mut output)?;
                ensure!(bytes <= MAX_BOOK_BYTES, "书籍超过 256 MB 导入上限");
                files.push(name.to_owned());
            }
            Ok(serde_json::to_value(Download { files })?)
        }
        _ => bail!("文件夹书源无需连接账户"),
    }
}

fn source_root(data_dir: &Path) -> Result<PathBuf> {
    let path = directory_settings(data_dir)?
        .directory
        .context("请在「插件 → 文件夹书源 → 设置」中选择书籍目录")?;
    ensure!(
        path.is_absolute() && path.is_dir(),
        "书籍目录不可用，请在插件设置中重新选择"
    );
    Ok(path.canonicalize()?)
}

fn directory_settings(data_dir: &Path) -> Result<DirectorySettings> {
    match File::open(data_dir.join("directory.json")) {
        Ok(file) => return Ok(serde_json::from_reader(file)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let config = data_dir.join("folder.txt");
    let legacy = match fs::read_to_string(&config).or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            fs::read_to_string("folder.txt")
        } else {
            Err(error)
        }
    }) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(DirectorySettings {
        directory: legacy
            .as_deref()
            .map(|text| text.trim_start_matches('\u{feff}').trim())
            .filter(|text| !text.is_empty())
            .map(PathBuf::from),
    })
}

fn resolve(root: &Path, id: &str) -> Result<PathBuf> {
    let mut path = root.to_owned();
    for part in id.split('/') {
        ensure!(
            !part.is_empty() && part != "." && part != ".." && !part.contains([':', '\\', '\0']),
            "无效的书籍标识"
        );
        path.push(part);
    }
    let path = path.canonicalize()?;
    ensure!(
        path.starts_with(root) && path != root,
        "书籍不在已配置的文件夹中"
    );
    Ok(path)
}

fn book_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("txt")
                || extension.eq_ignore_ascii_case("epub")
                || extension.eq_ignore_ascii_case("pdf")
        })
}

fn send(stdout: &mut impl Write, event: Event) -> Result<()> {
    serde_json::to_writer(&mut *stdout, &event)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}
