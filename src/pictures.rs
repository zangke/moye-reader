use std::{
    io::Cursor,
    path::Path,
    sync::{Arc, OnceLock},
};

use anyhow::{Result, ensure};
use gpui_kit::RenderImage;
use image::{Frame, ImageReader, Limits, Rgba, RgbaImage};
use smallvec::smallvec;

use crate::epub::{MAX_IMAGE, open_zip, read_entry};

pub struct Picture {
    pub art: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
    glyph: OnceLock<Arc<RenderImage>>,
}

impl Picture {
    pub fn from_rgba(mut bytes: Vec<u8>, width: u32, height: u32) -> Result<Self> {
        ensure!(width > 0 && height > 0, "图片尺寸无效");
        ensure!(
            bytes.len() == width as usize * height as usize * 4,
            "PDF 页面像素数据不完整"
        );
        // GPUI's render images store BGRA bytes.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let image = RgbaImage::from_raw(width, height, bytes)
            .ok_or_else(|| anyhow::anyhow!("无法创建 PDF 页面图像"))?;
        let art = Arc::new(RenderImage::new(smallvec![Frame::new(image)]));
        Ok(Self {
            art,
            width,
            height,
            glyph: OnceLock::new(),
        })
    }

    pub fn glyph(&self) -> Arc<RenderImage> {
        self.glyph
            .get_or_init(|| {
                let src = self.art.as_bytes(0).unwrap();
                let factor = (160. / self.width.max(self.height) as f32).min(1.);
                let width = (self.width as f32 * factor).round().max(1.) as u32;
                let height = (self.height as f32 * factor).round().max(1.) as u32;
                let source = |x, y| {
                    let index = 4 * (y as usize * self.width as usize + x as usize);
                    &src[index..index + 4]
                };
                let corners = [
                    source(0, 0),
                    source(self.width - 1, 0),
                    source(0, self.height - 1),
                    source(self.width - 1, self.height - 1),
                ];
                let transparent = corners.iter().any(|pixel| pixel[3] < 16);
                let dark_paper = corners
                    .iter()
                    .filter(|pixel| luminance(pixel) < 128)
                    .count()
                    >= 3;
                let frame = |r, g, b| {
                    Frame::new(RgbaImage::from_fn(width, height, |x, y| {
                        let pixel = source(x * self.width / width, y * self.height / height);
                        let coverage = if transparent {
                            255
                        } else if dark_paper {
                            luminance(pixel)
                        } else {
                            255 - luminance(pixel)
                        };
                        let alpha = (u16::from(pixel[3]) * coverage / 255) as u8;
                        Rgba([b, g, r, alpha])
                    }))
                };
                Arc::new(RenderImage::new(smallvec![
                    frame(39, 47, 42),
                    frame(212, 223, 214)
                ]))
            })
            .clone()
    }
}

fn luminance(bgra: &[u8]) -> u16 {
    (54 * u16::from(bgra[2]) + 183 * u16::from(bgra[1]) + 19 * u16::from(bgra[0])) >> 8
}

pub fn read_picture(archive: &Path, name: &str) -> Result<Picture> {
    let bytes = read_entry(&mut open_zip(archive)?, name, MAX_IMAGE)?;
    decode_picture(&bytes, 1380)
}

pub fn read_shelf_scene() -> Result<Picture> {
    decode_picture(include_bytes!("../assets/misty-coast.jpg"), 1680)
}

fn decode_picture(bytes: &[u8], max_side: u32) -> Result<Picture> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(96 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode()?;
    let mut image = if decoded.width().max(decoded.height()) > max_side {
        decoded.thumbnail(max_side, max_side).into_rgba8()
    } else {
        decoded.into_rgba8()
    };
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let (width, height) = image.dimensions();
    let art = Arc::new(RenderImage::new(smallvec![Frame::new(image)]));
    Ok(Picture {
        art,
        width,
        height,
        glyph: OnceLock::new(),
    })
}
