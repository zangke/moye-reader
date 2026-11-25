use std::{collections::HashMap, path::Path, sync::Mutex};

use anyhow::{Context, Result, bail, ensure};
use gpui_kit::SharedString;
use pdfium_render::prelude::{
    PdfBookmark, PdfDocument, PdfDocumentMetadataTagType, PdfRenderConfig, Pdfium,
};

use crate::{
    book::{Block, BlockKind, Book, BookInfo, ContentsEntry, Section},
    pictures::Picture,
};

pub const COVER: &str = "pdf:cover";
const PAGE_PREFIX: &str = "pdf:page:";
const PAGE_WIDTH: i32 = 1600;
const COVER_WIDTH: i32 = 600;
const MAX_PAGES: usize = 20_000;

static PDFIUM_LOCK: Mutex<()> = Mutex::new(());

fn with_engine<T>(action: impl FnOnce(&Pdfium) -> Result<T>) -> Result<T> {
    let _guard = PDFIUM_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("PDF 渲染组件状态异常"))?;
    let pdfium = pdfium_auto::bind_bundled()
        .map_err(|error| anyhow::anyhow!("无法载入 PDF 渲染组件：{error}"))?;
    action(&pdfium)
}

fn metadata(document: &PdfDocument<'_>, tag: PdfDocumentMetadataTagType) -> String {
    document
        .metadata()
        .get(tag)
        .map(|tag| tag.value().trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_default()
}

fn bookmark_page(bookmark: &PdfBookmark<'_>) -> Option<usize> {
    let page = bookmark
        .destination()
        .and_then(|target| target.page_index().ok());
    let page = page.or_else(|| {
        let action = bookmark.action()?;
        action
            .as_local_destination_action()?
            .destination()
            .ok()?
            .page_index()
            .ok()
    })?;
    Some(usize::from(page))
}

fn page_contents(pages: usize) -> Vec<ContentsEntry> {
    (0..pages)
        .map(|page| ContentsEntry {
            title: format!("第 {} 页", page + 1).into(),
            section: 0,
            block: page,
            depth: 0,
        })
        .collect()
}

fn outline_contents(document: &PdfDocument<'_>, pages: usize) -> Vec<ContentsEntry> {
    let mut depths = HashMap::new();
    let mut entries = Vec::new();
    for bookmark in document.bookmarks().iter() {
        let depth = bookmark
            .parent()
            .and_then(|parent| depths.get(&parent).copied())
            .map_or(0, |depth| depth + 1);
        depths.insert(bookmark.clone(), depth);

        let Some(title) = bookmark
            .title()
            .map(|title| title.trim().to_owned())
            .filter(|title| !title.is_empty() && !title.chars().any(char::is_control))
        else {
            continue;
        };
        let Some(page) = bookmark_page(&bookmark).filter(|page| *page < pages) else {
            continue;
        };
        entries.push(ContentsEntry {
            title: title.into(),
            section: 0,
            block: page,
            depth,
        });
    }
    entries
}

fn contents(document: &PdfDocument<'_>, pages: usize) -> Vec<ContentsEntry> {
    let outline = outline_contents(document, pages);
    if outline.is_empty() {
        page_contents(pages)
    } else {
        outline
    }
}

pub fn page_path(index: usize) -> String {
    format!("{PAGE_PREFIX}{index}")
}

fn page_index(path: &str) -> Result<usize> {
    path.strip_prefix(PAGE_PREFIX)
        .context("PDF 页面标识无效")?
        .parse()
        .context("PDF 页面标识无效")
}

pub fn parse(path: &Path, mut info: BookInfo) -> Result<Book> {
    with_engine(|pdfium| {
        let document = pdfium
            .load_pdf_from_file(path, None)
            .with_context(|| format!("无法读取 PDF「{}」", info.title))?;
        let pages = usize::from(document.pages().len());
        ensure!(pages > 0, "这份 PDF 没有可阅读的页面");
        ensure!(pages <= MAX_PAGES, "PDF 页数超过 {MAX_PAGES} 页的导入上限");

        let title = metadata(&document, PdfDocumentMetadataTagType::Title);
        if !title.is_empty() {
            info.title = title;
        }
        info.author = metadata(&document, PdfDocumentMetadataTagType::Author);
        info.cover = Some(COVER.to_owned());

        let blocks = (0..pages)
            .map(|page| Block {
                kind: BlockKind::Image {
                    path: page_path(page),
                    alt: String::new(),
                },
                ..Default::default()
            })
            .collect();
        let section = Section {
            title: SharedString::default(),
            blocks,
            ..Default::default()
        };
        let mut book = Book::new(info, vec![section]);
        book.contents = contents(&document, pages);
        Ok(book)
    })
}

pub fn render(path: &Path, asset: &str) -> Result<Picture> {
    let (page, width) = if asset == COVER {
        (0, COVER_WIDTH)
    } else {
        (page_index(asset)?, PAGE_WIDTH)
    };
    with_engine(|pdfium| {
        let document = pdfium
            .load_pdf_from_file(path, None)
            .context("无法打开 PDF")?;
        let page_number = page + 1;
        let page = document
            .pages()
            .get(u16::try_from(page).context("PDF 页码超出范围")?)
            .with_context(|| format!("无法渲染 PDF 第 {page_number} 页"))?;
        let config = PdfRenderConfig::new()
            .set_target_width(width)
            .set_maximum_height(width * 3)
            .render_form_data(true)
            .render_annotations(true);
        let bitmap = page
            .render_with_config(&config)
            .context("PDF 页面渲染失败")?;
        if bitmap.width() <= 0 || bitmap.height() <= 0 {
            bail!("PDF 页面尺寸无效");
        }
        Picture::from_rgba(
            bitmap.as_rgba_bytes(),
            bitmap.width() as u32,
            bitmap.height() as u32,
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::book::Format;

    fn assemble_pdf(objects: &[String], root: usize, info: usize) -> Vec<u8> {
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            write!(pdf, "{} 0 obj\n{}\nendobj\n", index + 1, object).unwrap();
        }
        let xref = pdf.len();
        write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
        for offset in offsets {
            writeln!(pdf, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            pdf,
            "trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1,
        )
        .unwrap();
        pdf
    }

    fn sample_pdf() -> Vec<u8> {
        let stream = "BT /F1 24 Tf 72 750 Td (Moye PDF) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Title (PDF test) /Author (Mo Ye) >>".to_owned(),
        ];
        assemble_pdf(&objects, 1, 6)
    }

    fn outlined_pdf() -> Vec<u8> {
        let stream = "BT /F1 24 Tf 72 750 Td (Chapter) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R /Outlines 8 0 R /PageMode /UseOutlines >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 6 0 R >> >> /Contents 7 0 R >>".to_owned(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
            "<< /Type /Outlines /First 9 0 R /Last 10 0 R /Count 2 >>".to_owned(),
            "<< /Title (Chapter One) /Parent 8 0 R /Next 10 0 R /Dest [3 0 R /Fit] >>".to_owned(),
            "<< /Title (Chapter Two) /Parent 8 0 R /Prev 9 0 R /First 11 0 R /Last 11 0 R /Count 1 /Dest [4 0 R /Fit] >>".to_owned(),
            "<< /Title (Section 2.1) /Parent 10 0 R /Dest [5 0 R /Fit] >>".to_owned(),
            "<< /Title (Outlined PDF) /Author (Mo Ye) >>".to_owned(),
        ];
        assemble_pdf(&objects, 1, 12)
    }

    fn pdf_info() -> BookInfo {
        BookInfo {
            id: "2".repeat(64),
            title: "测试".into(),
            author: String::new(),
            format: Format::Pdf,
            cover: None,
        }
    }

    #[test]
    fn reads_pdf_metadata_and_renders_the_first_page() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sample.pdf");
        std::fs::write(&path, sample_pdf()).unwrap();
        let book = parse(&path, pdf_info()).unwrap();

        assert_eq!(book.info.title, "PDF test");
        assert_eq!(book.info.author, "Mo Ye");
        assert_eq!(book.info.cover.as_deref(), Some(COVER));
        assert_eq!(book.sections.len(), 1);
        assert_eq!(book.sections[0].blocks.len(), 1);
        assert_eq!(book.title_at(0, 0).as_ref(), "第 1 页");
        assert_eq!(page_index(&page_path(0)).unwrap(), 0);

        let picture = render(&path, &page_path(0)).unwrap();
        assert!(picture.width > 0);
        assert!(picture.height > picture.width);
        let cover = render(&path, COVER).unwrap();
        assert!(cover.width <= COVER_WIDTH as u32);
    }

    #[test]
    fn uses_embedded_pdf_outline_for_chapter_navigation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("outlined.pdf");
        std::fs::write(&path, outlined_pdf()).unwrap();
        let book = parse(&path, pdf_info()).unwrap();

        assert_eq!(book.info.title, "Outlined PDF");
        assert_eq!(
            book.contents
                .iter()
                .map(|entry| (entry.title.as_ref(), entry.block, entry.depth))
                .collect::<Vec<_>>(),
            [
                ("Chapter One", 0, 0),
                ("Chapter Two", 1, 0),
                ("Section 2.1", 2, 1),
            ]
        );
        assert_eq!(book.title_at(0, 0).as_ref(), "Chapter One");
        assert_eq!(book.title_at(0, 1).as_ref(), "Chapter Two");
        assert_eq!(book.title_at(0, 2).as_ref(), "Section 2.1");
    }
}
