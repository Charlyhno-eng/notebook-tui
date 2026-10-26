use base64::{Engine, engine::general_purpose::STANDARD};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
};

/// Portable half-block graphics: works in ordinary true-color terminals, including tmux.
#[derive(Default)]
pub struct Images {
    cache: HashMap<(u64, usize), Vec<Line<'static>>>,
}
impl Images {
    pub fn render(&mut self, encoded: &str, width: usize) -> Vec<Line<'static>> {
        let mut hash = DefaultHasher::new();
        encoded.hash(&mut hash);
        let key = (hash.finish(), width);
        if let Some(lines) = self.cache.get(&key) {
            return lines.clone();
        }
        let result = self
            .decode(encoded, width)
            .unwrap_or_else(|e| vec![Line::raw(format!("Image: {e} — o: open in browser"))]);
        if self.cache.len() >= 64 {
            self.cache.clear();
        }
        self.cache.insert(key, result.clone());
        result
    }
    fn decode(&self, encoded: &str, width: usize) -> anyhow::Result<Vec<Line<'static>>> {
        anyhow::ensure!(
            encoded.len() < 48_000_000,
            "image too large for the terminal preview"
        );
        let bytes = STANDARD.decode(
            encoded
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>(),
        )?;
        let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(64 * 1024 * 1024);
        limits.max_image_width = Some(16384);
        limits.max_image_height = Some(16384);
        reader.limits(limits);
        let image = reader
            .decode()?
            .thumbnail(width.clamp(1, 160) as u32, 48)
            .to_rgba8();
        fn color(p: &image::Rgba<u8>) -> Color {
            let a = p[3] as u32;
            let mix = |v: u8| ((v as u32 * a + 255 * (255 - a)) / 255) as u8;
            Color::Rgb(mix(p[0]), mix(p[1]), mix(p[2]))
        }
        let mut lines = Vec::new();
        for y in (0..image.height()).step_by(2) {
            let mut spans = Vec::new();
            for x in 0..image.width() {
                let top = color(image.get_pixel(x, y));
                let bottom = if y + 1 < image.height() {
                    color(image.get_pixel(x, y + 1))
                } else {
                    Color::Rgb(15, 20, 30)
                };
                spans.push(Span::styled("▀", Style::default().fg(top).bg(bottom)));
            }
            lines.push(Line::from(spans));
        }
        Ok(lines)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn png_to_terminal_blocks() {
        let im = image::DynamicImage::new_rgb8(8, 8);
        let mut bytes = Cursor::new(Vec::new());
        im.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        let mut images = Images::default();
        let lines = images.render(&STANDARD.encode(bytes.into_inner()), 8);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].width(), 8);
        assert_eq!(lines[0].spans[0].content, "▀");
    }
}
