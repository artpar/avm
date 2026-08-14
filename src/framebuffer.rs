use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use image::{DynamicImage, ImageBuffer, Rgba};
use sha2::{Digest, Sha256};

pub const PIXMAN_X8R8G8B8: u32 = 0x2002_0888;
pub const PIXMAN_A8R8G8B8: u32 = 0x2002_8888;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Framebuffer {
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Framebuffer {
    pub fn from_scanout(
        width: u32,
        height: u32,
        stride: u32,
        format: u32,
        data: &[u8],
    ) -> Result<Self> {
        ensure!(
            width > 0 && height > 0,
            "scanout dimensions must be positive"
        );
        ensure!(
            stride >= width.saturating_mul(4),
            "scanout stride is too small"
        );
        let expected = (stride as usize)
            .checked_mul(height as usize)
            .context("scanout byte length overflow")?;
        ensure!(
            data.len() >= expected,
            "scanout has {} bytes, expected {expected}",
            data.len()
        );
        Ok(Self {
            width,
            height,
            stride,
            format,
            data: data[..expected].to_vec(),
        })
    }

    pub fn apply_update(
        &mut self,
        rect: Rect,
        update_stride: u32,
        format: u32,
        data: &[u8],
    ) -> Result<()> {
        ensure!(rect.x >= 0 && rect.y >= 0, "negative update origin");
        ensure!(
            rect.width > 0 && rect.height > 0,
            "update dimensions must be positive"
        );
        ensure!(
            format == self.format,
            "pixel format changed without a new scanout"
        );
        let right = (rect.x as u32)
            .checked_add(rect.width as u32)
            .context("update x overflow")?;
        let bottom = (rect.y as u32)
            .checked_add(rect.height as u32)
            .context("update y overflow")?;
        ensure!(
            right <= self.width && bottom <= self.height,
            "update rectangle [{},{},{},{}] outside scanout {}x{}",
            rect.x,
            rect.y,
            rect.width,
            rect.height,
            self.width,
            self.height
        );
        let row_bytes = (rect.width as usize)
            .checked_mul(4)
            .context("row length overflow")?;
        ensure!(
            update_stride as usize >= row_bytes,
            "update stride is too small"
        );
        let needed = (update_stride as usize)
            .checked_mul(rect.height as usize)
            .context("update byte length overflow")?;
        ensure!(
            data.len() >= needed,
            "update has {} bytes, expected {needed}",
            data.len()
        );

        for row in 0..rect.height as usize {
            let src = row * update_stride as usize;
            let dst = (rect.y as usize + row) * self.stride as usize + rect.x as usize * 4;
            self.data[dst..dst + row_bytes].copy_from_slice(&data[src..src + row_bytes]);
        }
        Ok(())
    }

    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn stride(&self) -> u32 {
        self.stride
    }
    pub fn format(&self) -> u32 {
        self.format
    }
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn sha256(&self) -> String {
        hex::encode(Sha256::digest(&self.data))
    }

    pub fn save_png(&self, path: impl AsRef<Path>) -> Result<()> {
        let bytes = self.png_bytes()?;
        std::fs::write(path.as_ref(), bytes)
            .with_context(|| format!("save PNG {}", path.as_ref().display()))
    }

    pub fn png_bytes(&self) -> Result<Vec<u8>> {
        if self.format != PIXMAN_X8R8G8B8 && self.format != PIXMAN_A8R8G8B8 {
            bail!(
                "PNG conversion does not support pixman format {:#010x}",
                self.format
            );
        }
        let mut rgba = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height as usize {
            let row = &self.data[y * self.stride as usize..][..self.width as usize * 4];
            for bgra in row.chunks_exact(4) {
                rgba.extend_from_slice(&[bgra[2], bgra[1], bgra[0], 255]);
            }
        }
        let image: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_raw(self.width, self.height, rgba).context("construct RGBA image")?;
        let mut bytes = std::io::Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .context("encode framebuffer PNG")?;
        Ok(bytes.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_copies_only_the_announced_rectangle_with_padded_strides() {
        let mut frame = Framebuffer::from_scanout(3, 2, 16, PIXMAN_X8R8G8B8, &[0; 32]).unwrap();
        let update = [1, 2, 3, 4, 5, 6, 7, 8, 99, 99, 99, 99];
        frame
            .apply_update(
                Rect {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 1,
                },
                12,
                PIXMAN_X8R8G8B8,
                &update,
            )
            .unwrap();
        assert_eq!(&frame.bytes()[20..28], &update[..8]);
        assert!(frame.bytes()[..20].iter().all(|byte| *byte == 0));
        assert!(frame.bytes()[28..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn rejects_updates_outside_the_current_scanout() {
        let mut frame = Framebuffer::from_scanout(2, 2, 8, PIXMAN_X8R8G8B8, &[0; 16]).unwrap();
        let error = frame
            .apply_update(
                Rect {
                    x: 1,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                8,
                PIXMAN_X8R8G8B8,
                &[0; 8],
            )
            .unwrap_err();
        assert!(error.to_string().contains("outside scanout"));
    }

    #[test]
    fn png_conversion_handles_little_endian_xrgb() {
        let frame = Framebuffer::from_scanout(1, 1, 4, PIXMAN_X8R8G8B8, &[1, 2, 3, 0]).unwrap();
        let temp = tempfile::NamedTempFile::new().unwrap();
        frame.save_png(temp.path()).unwrap();
        let bytes = std::fs::read(temp.path()).unwrap();
        let pixel = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .unwrap()
            .into_rgba8()
            .get_pixel(0, 0)
            .0;
        assert_eq!(pixel, [3, 2, 1, 255]);
    }
}
