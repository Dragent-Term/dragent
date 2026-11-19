use super::block::Block;

/// Ordered collection of `Block` entries for one terminal pane.
pub struct BlockList {
    blocks: Vec<Block>,
    next_id: usize,

    /// Bootstrap prompt lines shown before the first command.
    pub bootstrap_lines: Vec<crate::console::escape_parse::ColoredLine>,
    pub bootstrap_height_px: f32,
}

impl BlockList {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            next_id: 0,
            bootstrap_lines: Vec::new(),
            bootstrap_height_px: 0.0,
        }
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn get(&self, id: usize) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn get_mut(&mut self, id: usize) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    pub fn index_of(&self, id: usize) -> Option<usize> {
        self.blocks.iter().position(|b| b.id == id)
    }

    pub fn begin(&mut self, block: Block) -> usize {
        let id = block.id;
        self.blocks.push(block);
        id
    }

    pub fn next_id(&mut self) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn toggle_block_expansion(&mut self, block_id: usize) -> bool {
        if let Some(block) = self.get_mut(block_id) {
            block.expanded = !block.expanded;
            true
        } else {
            false
        }
    }

    pub fn restore(&mut self, blocks: Vec<Block>) {
        self.blocks = blocks;
        self.next_id = self.blocks.iter().map(|b| b.id).max().unwrap_or(0) + 1;
    }

    pub fn remove_block_by_id(&mut self, id: usize) {
        if let Some(pos) = self.blocks.iter().position(|b| b.id == id) {
            self.blocks.remove(pos);
        }
    }

    /// Wipe all command history for this pane (used by `clear` / tab reset).
    pub fn clear_history(&mut self) {
        self.blocks.clear();
        self.next_id = 0;
        self.bootstrap_lines.clear();
        self.bootstrap_height_px = 0.0;
    }
}

impl Default for BlockList {
    fn default() -> Self {
        Self::new()
    }
}
