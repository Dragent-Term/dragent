use std::collections::HashMap;
use warpui::fonts::{FontId, GlyphId};

#[derive(Default)]
pub struct CellGlyphCache {
    cache: HashMap<(char, FontId), Option<(GlyphId, FontId)>>,
}

impl CellGlyphCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(
        &mut self,
        font_cache: &warpui::fonts::Cache,
        font_id: FontId,
        ch: char,
    ) -> Option<(GlyphId, FontId)> {
        *self
            .cache
            .entry((ch, font_id))
            .or_insert_with(|| font_cache.glyph_for_char(font_id, ch, true))
    }
}
