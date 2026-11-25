use std::{collections::HashMap, num::NonZeroUsize, ops::Range, sync::LazyLock};

use anyhow::{Result, bail};
use encoding_rs::{GB18030, UTF_16BE, UTF_16LE};
use gpui_kit::SharedString;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Format {
    Txt,
    Epub,
    Pdf,
}

impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Epub => "epub",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BookInfo {
    pub id: String,
    pub title: String,
    pub author: String,
    pub format: Format,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PointInText {
    pub block: usize,
    pub byte: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    pub section: usize,
    pub start: PointInText,
    pub end: PointInText,
}

impl Passage {
    pub fn new(section: usize, a: PointInText, b: PointInText) -> Option<Self> {
        (a != b).then(|| Self {
            section,
            start: a.min(b),
            end: a.max(b),
        })
    }

    pub fn in_block(&self, section: usize, block: usize, len: usize) -> Option<Range<usize>> {
        if section != self.section || block < self.start.block || block > self.end.block {
            return None;
        }
        let start = if block == self.start.block {
            self.start.byte.min(len)
        } else {
            0
        };
        let end = if block == self.end.block {
            self.end.byte.min(len)
        } else {
            len
        };
        (start < end).then_some(start..end)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Target {
    pub path: String,
    pub fragment: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Link {
    Local(Target),
    External(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub marked: bool,
    pub note_ref: bool,
    pub note_kind: Option<NoteKind>,
    pub color: Option<u32>,
    pub link: Option<Link>,
    pub glyph: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Inline,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum BlockKind {
    #[default]
    Paragraph,
    Heading(u8),
    Quote,
    Verse,
    Code,
    List,
    Rule,
    Image {
        path: String,
        alt: String,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Block {
    pub text: SharedString,
    pub kind: BlockKind,
    pub spans: Vec<Span>,
    pub quote_group: Option<NonZeroUsize>,
}

impl Block {
    pub fn paragraph(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

#[derive(Default)]
pub struct Section {
    pub title: SharedString,
    pub blocks: Vec<Block>,
    pub header: Option<usize>,
    pub notes: Vec<Target>,
    pub note_locations: HashMap<Target, usize>,
    pub legacy_rows: Vec<LegacyRow>,
}

#[derive(Clone, Copy)]
pub struct LegacyRow {
    pub start: PointInText,
    pub len: usize,
}

impl Section {
    pub fn remap_legacy_point(&self, point: PointInText) -> Option<PointInText> {
        let row = self.legacy_rows.get(point.block)?;
        let mut block = row.start.block;
        let mut byte = row.start.byte + point.byte.min(row.len);
        loop {
            let text = &self.blocks.get(block)?.text;
            if byte <= text.len() {
                return Some(PointInText { block, byte });
            }
            byte -= text.len();
            block += 1;
        }
    }
}

#[derive(Clone)]
pub struct ContentsEntry {
    pub title: SharedString,
    pub section: usize,
    pub block: usize,
    pub depth: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Footnote,
    Endnote,
    Gloss,
    Commentary,
    Variant,
}

impl NoteKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Footnote => "脚注",
            Self::Endnote => "尾注",
            Self::Gloss => "词注",
            Self::Commentary => "评注",
            Self::Variant => "校记",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceNote {
    pub title: SharedString,
    pub text: SharedString,
    pub kind: NoteKind,
}

pub const DEMO_ID: &str = "moye-demo";

pub struct Book {
    pub info: BookInfo,
    pub sections: Vec<Section>,
    pub contents: Vec<ContentsEntry>,
    pub notes: HashMap<Target, SourceNote>,
    pub destinations: HashMap<Target, (usize, usize)>,
    pub skipped_sections: usize,
    pub linear: Vec<usize>,
    prefix: Vec<usize>,
    units: Vec<Vec<usize>>,
}

impl Book {
    pub fn new(info: BookInfo, sections: Vec<Section>) -> Self {
        let linear = (0..sections.len()).collect::<Vec<_>>();
        let mut book = Self {
            info,
            sections,
            linear: vec![],
            contents: vec![],
            notes: HashMap::new(),
            destinations: HashMap::new(),
            skipped_sections: 0,
            prefix: vec![],
            units: vec![],
        };
        book.set_linear(linear);
        book
    }

    pub fn set_linear(&mut self, linear: Vec<usize>) {
        self.units = self
            .sections
            .iter()
            .map(|section| {
                let mut total = 0;
                std::iter::once(0)
                    .chain(section.blocks.iter().enumerate().map(|(ix, block)| {
                        total += if section.header == Some(ix) {
                            0
                        } else if matches!(block.kind, BlockKind::Image { .. }) {
                            180
                        } else {
                            block.text.len().max(1)
                        };
                        total
                    }))
                    .collect()
            })
            .collect();
        let mut total = 0;
        self.prefix = vec![0; self.sections.len()];
        for &ix in &linear {
            self.prefix[ix] = total;
            total += self.units[ix].last().copied().unwrap_or_default().max(1);
        }
        self.prefix.push(total);
        self.linear = linear;
    }

    pub fn contents_index(&self, section: usize, block: usize) -> Option<usize> {
        let local = || {
            self.contents
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.section == section)
        };
        if let Some((index, _)) = local()
            .filter(|(_, entry)| entry.block <= block)
            .max_by_key(|(_, entry)| entry.block)
            .or_else(|| local().min_by_key(|(_, entry)| entry.block))
        {
            return Some(index);
        }
        self.linear.binary_search(&section).ok()?;
        self.contents
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.section < section && self.linear.binary_search(&entry.section).is_ok()
            })
            .max_by_key(|(_, entry)| (entry.section, entry.block))
            .map(|(index, _)| index)
    }

    pub fn title_at(&self, section: usize, block: usize) -> SharedString {
        if let Some(index) = self.contents_index(section, block) {
            self.contents[index].title.clone()
        } else {
            let title = &self.sections[section].title;
            if title.is_empty() {
                self.info.title.clone().into()
            } else {
                title.clone()
            }
        }
    }

    pub fn neighboring_chapter(
        &self,
        section: usize,
        block: usize,
        forward: bool,
    ) -> Option<&ContentsEntry> {
        let Some(index) = self.contents_index(section, block) else {
            return if forward {
                self.contents.iter().find(|entry| entry.section > section)
            } else {
                None
            };
        };
        let current = &self.contents[index];
        let another = |entry: &&ContentsEntry| {
            (entry.section, entry.block) != (current.section, current.block)
        };
        if forward {
            self.contents[index + 1..].iter().find(another)
        } else {
            self.contents[..index].iter().rev().find(another)
        }
    }

    pub fn progress(&self, section: usize, item: usize, byte: usize, at_end: bool) -> f32 {
        let total = self.prefix.last().copied().unwrap_or(1).max(1);
        let base = self.prefix[section];
        let ends = &self.units[section];
        let end = ends.last().copied().unwrap_or_default();
        let within = if at_end {
            end
        } else {
            ends.get(item.saturating_sub(1))
                .copied()
                .unwrap_or(end)
                .saturating_add(byte)
                .min(end)
        };
        ((base + within) as f32 / total as f32).clamp(0., 1.)
    }

    pub fn quote(&self, passage: &Passage) -> String {
        let Some(section) = self.sections.get(passage.section) else {
            return String::new();
        };
        let mut parts = vec![];
        for (ix, block) in section
            .blocks
            .iter()
            .enumerate()
            .skip(passage.start.block)
            .take(
                passage
                    .end
                    .block
                    .saturating_sub(passage.start.block)
                    .saturating_add(1),
            )
        {
            if let Some(text) = passage
                .in_block(passage.section, ix, block.text.len())
                .and_then(|range| block.text.get(range))
            {
                parts.push(text);
            }
        }
        parts.join("\n")
    }

    pub fn restore_passage(&self, old: &Passage, quote: &str) -> Option<Passage> {
        if quote.is_empty() {
            return None;
        }
        if self.quote(old) == quote {
            return Some(old.clone());
        }
        let section = self.sections.get(old.section)?;
        let first = old.start.block.saturating_sub(32);
        let last = old.end.block.saturating_add(33).min(section.blocks.len());
        let mut text = String::new();
        let mut starts = Vec::new();
        for (block, item) in section.blocks.get(first..last)?.iter().enumerate() {
            if item.text.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            starts.push((block + first, text.len()));
            text.push_str(&item.text);
        }
        let mut matches = text.match_indices(quote);
        let (start, _) = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        let end = start + quote.len();
        let point = |index| {
            let &(block, offset) = starts.get(
                starts
                    .partition_point(|(_, start)| *start <= index)
                    .checked_sub(1)?,
            )?;
            Some(PointInText {
                block,
                byte: index - offset,
            })
        };
        Passage::new(old.section, point(start)?, point(end)?)
    }
}

pub fn decode_text(bytes: &[u8]) -> Result<String> {
    let (encoding, bytes) = if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        (UTF_16LE, rest)
    } else if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        (UTF_16BE, rest)
    } else {
        let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
        if let Ok(text) = std::str::from_utf8(bytes) {
            if !text.contains('\0') {
                return Ok(text.to_owned());
            }
            bail!("此 TXT 含有无法读取的二进制内容");
        }
        (GB18030, bytes)
    };
    let (decoded, errors) = encoding.decode_without_bom_handling(bytes);
    if errors || decoded.contains('\0') {
        bail!("无法识别文本编码，请使用 UTF-8、带 BOM 的 UTF-16 或 GB18030");
    }
    Ok(decoded.into_owned())
}

static CHAPTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:第[零〇一二三四五六七八九十百千万两壹贰叁肆伍陆柒捌玖拾0-9０-９]{1,12}[章节節卷部回幕篇]|(?:chapter|part|book)\s+(?:[0-9]+|[ivxlcdm]+)\b|(?:序章|序言|引子|楔子|尾声|后记|番外)(?:$|[\s：:·]))").unwrap()
});

pub fn parse_txt(info: BookInfo, bytes: &[u8]) -> Result<Book> {
    let text = decode_text(bytes)?;
    let mut sections = vec![];
    let mut current = Section {
        title: "正文".into(),
        ..Default::default()
    };
    let mut physical_lines = 0;
    let flush = |section: &mut Section, sections: &mut Vec<Section>, title: SharedString| {
        let next = Section {
            title,
            ..Default::default()
        };
        if !section.blocks.is_empty() || section.title.as_ref() != "正文" {
            sections.push(std::mem::replace(section, next));
        } else {
            *section = next;
        }
    };
    for line in normalized_lines(&text) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.chars().take(81).count() <= 80 && CHAPTER.is_match(line) {
            flush(&mut current, &mut sections, line.to_owned().into());
            physical_lines = 0;
            continue;
        }
        if physical_lines >= 450 {
            let title = format!("续读 · {:02}", sections.len() + 2);
            flush(&mut current, &mut sections, title.into());
            physical_lines = 0;
        }
        for chunk in split_block(line, 4096) {
            current.blocks.push(Block::paragraph(chunk.to_owned()));
        }
        physical_lines += 1;
    }
    if !current.blocks.is_empty() {
        sections.push(current);
    }
    sections.retain(|section| !section.blocks.is_empty());
    if sections.is_empty() {
        bail!("这份 TXT 没有可阅读的正文");
    }
    let mut book = Book::new(info, sections);
    book.contents = book
        .sections
        .iter()
        .enumerate()
        .map(|(section, data)| ContentsEntry {
            title: data.title.clone(),
            section,
            block: 0,
            depth: 0,
        })
        .collect();
    Ok(book)
}

fn normalized_lines(text: &str) -> impl Iterator<Item = &str> {
    let mut remainder = text;
    std::iter::from_fn(move || {
        if remainder.is_empty() {
            return None;
        }
        let end = remainder.find(['\n', '\r']).unwrap_or(remainder.len());
        let (line, tail) = remainder.split_at(end);
        remainder = tail
            .strip_prefix("\r\n")
            .or_else(|| tail.strip_prefix(['\r', '\n']))
            .unwrap_or(tail);
        Some(line)
    })
}

pub fn split_block(mut text: &str, max_bytes: usize) -> impl Iterator<Item = &str> {
    std::iter::from_fn(move || {
        if text.is_empty() {
            return None;
        }
        let mut end = text.len().min(max_bytes);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len() {
            let mut from = end / 2;
            while !text.is_char_boundary(from) {
                from -= 1;
            }
            if let Some((ix, ch)) = text[from..end]
                .char_indices()
                .rev()
                .find(|(_, ch)| matches!(ch, '。' | '！' | '？' | ' ' | '\n'))
            {
                end = from + ix + ch.len_utf8();
            }
        }
        let (head, tail) = text.split_at(end);
        text = tail;
        Some(head)
    })
}
