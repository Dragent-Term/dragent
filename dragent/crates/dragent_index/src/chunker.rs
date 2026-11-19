use super::merkle::Fragment;
use super::symbols::extract_symbols;
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct Chunker;

impl Chunker {
    /// Target chunking: split text into chunks of target ~400 lines or 24 KiB, whichever is smaller.
    pub fn chunk(path: &Path, content: &str) -> Vec<Fragment> {
        let mut fragments = Vec::new();
        let bytes = content.as_bytes();
        let total_bytes = bytes.len();

        if total_bytes == 0 {
            let content_hash = hex::encode(Sha256::digest(b""));
            let fragment_id = format!("{}:0:0", content_hash);
            fragments.push(Fragment {
                fragment_id,
                path: path.to_path_buf(),
                byte_start: 0,
                byte_end: 0,
                content_hash,
                symbols: vec![],
            });
            return fragments;
        }

        let mut lines_indices = Vec::new();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                lines_indices.push((start, i + 1));
                start = i + 1;
            }
        }
        if start < total_bytes {
            lines_indices.push((start, total_bytes));
        }

        let max_lines = 400;
        let max_bytes = 24 * 1024;

        let mut current_chunk_start = 0;
        let mut current_line_count = 0;

        for (i, &(l_start, l_end)) in lines_indices.iter().enumerate() {
            let chunk_bytes_if_add = l_end - current_chunk_start;
            let count_if_add = current_line_count + 1;

            if (count_if_add > max_lines || chunk_bytes_if_add > max_bytes)
                && current_line_count > 0
            {
                // Flush previous chunk
                let chunk_end = lines_indices[i - 1].1;
                let chunk_data = &bytes[current_chunk_start..chunk_end];
                let content_hash = hex::encode(Sha256::digest(chunk_data));
                let fragment_id = format!("{}:{}:{}", content_hash, current_chunk_start, chunk_end);
                let chunk_text = std::str::from_utf8(chunk_data).unwrap_or("");
                let symbols = extract_symbols(path, chunk_text);
                fragments.push(Fragment {
                    fragment_id,
                    path: path.to_path_buf(),
                    byte_start: current_chunk_start,
                    byte_end: chunk_end,
                    content_hash,
                    symbols,
                });

                current_chunk_start = l_start;
                current_line_count = 1;
            } else {
                current_line_count += 1;
            }
        }

        // Flush remaining
        if current_line_count > 0 {
            let chunk_end = total_bytes;
            let chunk_data = &bytes[current_chunk_start..chunk_end];
            let content_hash = hex::encode(Sha256::digest(chunk_data));
            let fragment_id = format!("{}:{}:{}", content_hash, current_chunk_start, chunk_end);
            let chunk_text = std::str::from_utf8(chunk_data).unwrap_or("");
            let symbols = extract_symbols(path, chunk_text);
            fragments.push(Fragment {
                fragment_id,
                path: path.to_path_buf(),
                byte_start: current_chunk_start,
                byte_end: chunk_end,
                content_hash,
                symbols,
            });
        }

        fragments
    }

    /// Check if the first 8192 bytes contain a NUL character, indicating it is binary.
    pub fn is_binary(content: &[u8]) -> bool {
        let limit = content.len().min(8192);
        content[..limit].contains(&0)
    }
}
