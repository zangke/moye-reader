use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use moye_plugin_api::MAX_BOOK_BYTES as MAX_BOOK;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::{
    book::{Book, BookInfo, DEMO_ID, Format, Passage, PointInText, parse_txt},
    demo,
    epub::parse_epub,
    pdf,
};

#[derive(Clone, Serialize, Deserialize)]
pub struct ShelfFolder {
    pub id: u64,
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paper {
    #[default]
    #[serde(rename = "Paper")]
    Warm,
    White,
    Night,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tone {
    #[default]
    Amber,
    Sage,
    Rose,
}

impl Tone {
    pub const ALL: [Self; 3] = [Self::Amber, Self::Sage, Self::Rose];

    pub fn title(self) -> &'static str {
        match self {
            Self::Amber => "杏黄",
            Self::Sage => "苔绿",
            Self::Rose => "浅绯",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub section: usize,
    pub item: usize,
    #[serde(default)]
    pub byte: usize,
    pub offset: f32,
    pub progress: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub section: usize,
    pub item: usize,
    #[serde(default)]
    pub byte: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingMode {
    #[default]
    Scroll,
    Paged,
    Spread,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    pub id: u64,
    pub passage: Passage,
    pub quote: String,
    pub body: String,
    pub tone: Tone,
    pub created: u64,
}

impl Annotation {
    pub fn new(id: u64, passage: Passage, quote: String, body: String, tone: Tone) -> Self {
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            passage,
            quote,
            body,
            tone,
            created,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Reading {
    pub position: Position,
    pub annotations: Vec<Annotation>,
    pub bookmarks: Vec<Bookmark>,
    glyph_anchors: bool,
}

impl Reading {
    pub fn migrate_glyph_anchors(&mut self, book: &Book) {
        if std::mem::replace(&mut self.glyph_anchors, true) {
            return;
        }
        let remap = |section: usize, item: usize, byte| {
            book.sections.get(section)?.remap_legacy_point(PointInText {
                block: item.checked_sub(1)?,
                byte,
            })
        };
        if let Some(point) = remap(
            self.position.section,
            self.position.item,
            self.position.byte,
        ) {
            if point.byte != self.position.byte {
                self.position.offset = 0.;
            }
            self.position.item = point.block + 1;
            self.position.byte = point.byte;
        }
        for mark in &mut self.bookmarks {
            if let Some(point) = remap(mark.section, mark.item, mark.byte) {
                mark.item = point.block + 1;
                mark.byte = point.byte;
            }
        }
        for note in &mut self.annotations {
            let Some(section) = book.sections.get(note.passage.section) else {
                continue;
            };
            if let (Some(start), Some(end)) = (
                section.remap_legacy_point(note.passage.start),
                section.remap_legacy_point(note.passage.end),
            ) && let Some(passage) = Passage::new(note.passage.section, start, end)
            {
                let quote = book.quote(&passage);
                if !quote.is_empty() {
                    note.passage = passage;
                    note.quote = quote;
                }
            }
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(default)]
pub struct Settings {
    pub reading_mode: ReadingMode,
    pub handheld_reading_mode: ReadingMode,
    pub paper: Paper,
    pub size: u16,
    pub sidebar_open: bool,
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub disabled_plugins: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            reading_mode: ReadingMode::Scroll,
            handheld_reading_mode: ReadingMode::Paged,
            paper: Paper::Warm,
            size: 19,
            sidebar_open: true,
            disabled_plugins: BTreeSet::new(),
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Library {
    pub books: Vec<BookInfo>,
    pub active: Option<String>,
    pub reading: HashMap<String, Reading>,
    pub settings: Settings,
    pub next_annotation: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<ShelfFolder>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub book_folders: HashMap<String, u64>,
    pub next_folder: u64,
}

impl Library {
    pub fn folder(&self, id: u64) -> Option<&ShelfFolder> {
        self.folders.iter().find(|folder| folder.id == id)
    }

    pub fn name_folder(&mut self, id: Option<u64>, name: &str) -> Result<u64> {
        let name = name.trim();
        ensure!(!name.is_empty(), "请输入文件夹名称");
        ensure!(
            name.chars().count() <= 64 && !name.chars().any(char::is_control),
            "名称最多 64 个字符，且不能包含换行等控制字符"
        );
        let normalized = name.to_lowercase();
        ensure!(
            !self.folders.iter().any(|folder| {
                Some(folder.id) != id && folder.name.to_lowercase() == normalized
            }),
            "已有同名文件夹"
        );
        if let Some(id) = id {
            let folder = self
                .folders
                .iter_mut()
                .find(|folder| folder.id == id)
                .context("找不到这个文件夹")?;
            folder.name = name.to_owned();
            return Ok(id);
        }
        let greatest = self
            .folders
            .iter()
            .map(|folder| folder.id)
            .max()
            .unwrap_or_default();
        let id = self
            .next_folder
            .max(greatest)
            .checked_add(1)
            .context("无法分配文件夹编号")?;
        self.next_folder = id;
        self.folders.push(ShelfFolder {
            id,
            name: name.to_owned(),
        });
        Ok(id)
    }

    pub fn move_books(&mut self, books: &HashSet<String>, folder: Option<u64>) -> Result<()> {
        ensure!(
            self.books
                .iter()
                .filter(|info| books.contains(&info.id))
                .count()
                == books.len(),
            "所选书籍已发生变化"
        );
        if let Some(id) = folder {
            ensure!(self.folder(id).is_some(), "找不到这个文件夹");
            self.book_folders
                .extend(books.iter().map(|book| (book.clone(), id)));
        } else {
            self.book_folders.retain(|id, _| !books.contains(id));
        }
        Ok(())
    }

    pub fn remove_folder(&mut self, id: u64) {
        self.folders.retain(|folder| folder.id != id);
        self.book_folders.retain(|_, folder| *folder != id);
    }
}

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new() -> Result<Self> {
        let root = std::env::var_os("MOYE_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::data_local_dir().map(|root| root.join("Moye Reader")))
            .context("系统未提供应用数据目录")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn books_dir(&self) -> PathBuf {
        self.root.join("books")
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("library.json")
    }

    pub fn load(&self) -> Result<Library> {
        let path = self.state_path();
        let file = match File::open(path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Library::default()),
            Err(err) => return Err(err.into()),
        };
        let mut data: Library = serde_json::from_reader(file).context("书库记录无法读取")?;
        data.settings.size = data.settings.size.clamp(15, 31);
        let folders = data
            .folders
            .iter()
            .map(|folder| folder.id)
            .collect::<std::collections::HashSet<_>>();
        let books = data
            .books
            .iter()
            .map(|book| book.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        data.book_folders
            .retain(|book, folder| folders.contains(folder) && books.contains(book.as_str()));
        let lower_bound = data
            .reading
            .values()
            .flat_map(|reading| &reading.annotations)
            .map(|note| note.id)
            .max()
            .unwrap_or_default();
        data.next_annotation = data.next_annotation.max(lower_bound);
        Ok(data)
    }

    pub fn save(&self, state: &Library) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let mut tmp = NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut tmp, state)?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.state_path())?;
        Ok(())
    }

    pub fn path_for(&self, info: &BookInfo) -> Option<PathBuf> {
        if info.id.len() != 64 || !info.id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        Some(
            self.books_dir()
                .join(format!("{}.{}", info.id, info.format.extension())),
        )
    }

    pub fn read_book(&self, info: BookInfo) -> Result<Book> {
        if info.id == DEMO_ID {
            return Ok(demo::book());
        }
        let path = self.path_for(&info).context("书籍的本地标识无效")?;
        read_book(info, &path)
    }

    pub fn import(&self, path: &Path) -> Result<Book> {
        let format = match path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("txt") => Format::Txt,
            Some("epub") => Format::Epub,
            Some("pdf") => Format::Pdf,
            _ => bail!("请选择 TXT、EPUB 或 PDF 文件"),
        };
        let mut source = File::open(path)?;
        let metadata = source.metadata()?;
        ensure!(metadata.is_file(), "请选择一个书籍文件");
        ensure!(metadata.len() <= MAX_BOOK, "书籍超过 256 MB 的导入上限");

        let dir = self.books_dir();
        fs::create_dir_all(&dir)?;
        let mut tmp = NamedTempFile::new_in(dir)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0; 64 * 1024];
        let mut total = 0;
        loop {
            let len = source.read(&mut buffer)?;
            if len == 0 {
                break;
            }
            total += len as u64;
            ensure!(total <= MAX_BOOK, "书籍超过 256 MB 的导入上限");
            hasher.update(&buffer[..len]);
            tmp.write_all(&buffer[..len])?;
        }
        let mut id = hasher.finalize().to_hex().to_string();
        // Keep files with identical bytes but different declared formats distinct.
        id.replace_range(
            ..1,
            match format {
                Format::Txt => "0",
                Format::Epub => "1",
                Format::Pdf => "2",
            },
        );
        let title = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("未命名书籍")
            .to_owned();
        let info = BookInfo {
            id,
            title,
            author: String::new(),
            format,
            cover: None,
        };
        let dest = self.path_for(&info).context("生成书籍路径失败")?;
        let book = read_book(info, tmp.path())?;
        if !dest.exists() {
            tmp.as_file().sync_all()?;
            tmp.persist(dest)?;
        }
        Ok(book)
    }
}

fn read_book(info: BookInfo, path: &Path) -> Result<Book> {
    match info.format {
        Format::Txt => {
            let mut bytes = vec![];
            File::open(path)?
                .take(MAX_BOOK + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() as u64 <= MAX_BOOK, "TXT 超过可载入大小");
            parse_txt(info, &bytes)
        }
        Format::Epub => parse_epub(info, path),
        Format::Pdf => pdf::parse(path, info),
    }
}
