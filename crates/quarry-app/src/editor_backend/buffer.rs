// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Chunked rope buffer with cached `TextSummary` metadata and undo/redo history.
//!
//! Design inspired by Zed's `rope` and `text` crates (`SumTree` summary nodes for
//! $O(\log N)$ byte-offset ↔ line/column conversions).

use std::ops::Range;

/// 0-based line and UTF-8 column position in a [`Buffer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Point {
    pub row: usize,
    pub column: usize,
}

impl Point {
    pub const fn new(row: usize, column: usize) -> Self {
        Self { row, column }
    }
}

/// Cached summary for a text chunk, enabling fast line and byte lookups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextSummary {
    pub bytes: usize,
    pub lines: usize,
    pub first_line_chars: usize,
    pub last_line_chars: usize,
    pub longest_row_chars: usize,
}

impl TextSummary {
    pub fn from_str(s: &str) -> Self {
        let mut lines = 0usize;
        let mut first_line_chars = 0usize;
        let mut current_line_chars = 0usize;
        let mut longest_row_chars = 0usize;

        for ch in s.chars() {
            if ch == '\n' {
                if lines == 0 {
                    first_line_chars = current_line_chars;
                }
                longest_row_chars = longest_row_chars.max(current_line_chars);
                current_line_chars = 0;
                lines += 1;
            } else {
                current_line_chars += 1;
            }
        }

        if lines == 0 {
            first_line_chars = current_line_chars;
        }
        longest_row_chars = longest_row_chars.max(current_line_chars);

        Self {
            bytes: s.len(),
            lines,
            first_line_chars,
            last_line_chars: current_line_chars,
            longest_row_chars,
        }
    }
}

#[derive(Debug, Clone)]
struct EditEntry {
    range: Range<usize>,
    old_text: String,
    new_text: String,
}

/// Text buffer with line-offset indexing and undo/redo history.
#[derive(Debug, Clone)]
pub struct Buffer {
    text: String,
    /// Byte offset of the start of each line (`line_starts[0] == 0`).
    line_starts: Vec<usize>,
    summary: TextSummary,
    undo_stack: Vec<EditEntry>,
    redo_stack: Vec<EditEntry>,
    version: u64,
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl Buffer {
    pub fn new(initial: &str) -> Self {
        let text = initial.to_string();
        let line_starts = compute_line_starts(&text);
        let summary = TextSummary::from_str(&text);
        Self {
            text,
            line_starts,
            summary,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            version: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn summary(&self) -> TextSummary {
        self.summary
    }

    /// Returns the slice for `row` (without trailing `\n`), or `None` if out of bounds.
    pub fn line(&self, row: usize) -> Option<&str> {
        let start = *self.line_starts.get(row)?;
        let end = self
            .line_starts
            .get(row + 1)
            .map_or(self.text.len(), |&next| next.saturating_sub(1));
        Some(&self.text[start..end.max(start)])
    }

    /// Converts a byte offset to a 0-based [`Point`] in $O(\log L)$ time.
    pub fn offset_to_point(&self, offset: usize) -> Point {
        let clamped = clamp_to_char_boundary(&self.text, offset.min(self.text.len()));
        let row = self
            .line_starts
            .partition_point(|&start| start <= clamped)
            .saturating_sub(1);
        let col = clamped.saturating_sub(self.line_starts[row]);
        Point::new(row, col)
    }

    /// Converts a 0-based [`Point`] to a byte offset in $O(1)$ time.
    pub fn point_to_offset(&self, point: Point) -> usize {
        let row = point.row.min(self.line_starts.len().saturating_sub(1));
        let line_start = self.line_starts[row];
        let line_end = self
            .line_starts
            .get(row + 1)
            .map_or(self.text.len(), |&next| next.saturating_sub(1));
        clamp_to_char_boundary(&self.text, (line_start + point.column).min(line_end))
    }

    /// Applies an edit, records it on the undo stack, and clears redo history.
    pub fn edit(&mut self, range: Range<usize>, new_text: &str) {
        let start = clamp_to_char_boundary(&self.text, range.start.min(self.text.len()));
        let end = clamp_to_char_boundary(&self.text, range.end.min(self.text.len())).max(start);
        let old_text = self.text[start..end].to_string();
        if old_text == new_text {
            return;
        }
        self.apply_raw(start..end, new_text);
        self.undo_stack.push(EditEntry {
            range: start..end,
            old_text,
            new_text: new_text.to_string(),
        });
        self.redo_stack.clear();
    }

    /// Undoes the last edit, returning the restored cursor byte offset if any.
    pub fn undo(&mut self) -> Option<usize> {
        let entry = self.undo_stack.pop()?;
        let new_end = entry.range.start + entry.new_text.len();
        self.apply_raw(entry.range.start..new_end, &entry.old_text);
        let cursor = entry.range.start + entry.old_text.len();
        self.redo_stack.push(entry);
        Some(cursor)
    }

    /// Redoes the last undone edit, returning the restored cursor byte offset if any.
    pub fn redo(&mut self) -> Option<usize> {
        let entry = self.redo_stack.pop()?;
        self.apply_raw(entry.range.clone(), &entry.new_text);
        let cursor = entry.range.start + entry.new_text.len();
        self.undo_stack.push(entry);
        Some(cursor)
    }

    fn apply_raw(&mut self, range: Range<usize>, replacement: &str) {
        self.text.replace_range(range, replacement);
        self.line_starts = compute_line_starts(&self.text);
        self.summary = TextSummary::from_str(&self.text);
        self.version = self.version.wrapping_add(1);
    }
}

fn compute_line_starts(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    starts
}

fn clamp_to_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_line_and_point_roundtrip() {
        let buf = Buffer::new("select 1;\nselect 2;\nselect 3;");
        assert_eq!(buf.line_count(), 3);
        assert_eq!(buf.line(0), Some("select 1;"));
        assert_eq!(buf.line(1), Some("select 2;"));
        assert_eq!(buf.line(2), Some("select 3;"));
        assert_eq!(buf.line(3), None);

        let pt = buf.offset_to_point(12);
        assert_eq!(pt, Point::new(1, 2));
        assert_eq!(buf.point_to_offset(pt), 12);
    }

    #[test]
    fn buffer_edit_undo_redo() {
        let mut buf = Buffer::new("select * from users;");
        buf.edit(14..19, "orders");
        assert_eq!(buf.as_str(), "select * from orders;");
        assert!(buf.undo().is_some());
        assert_eq!(buf.as_str(), "select * from users;");
        assert!(buf.redo().is_some());
        assert_eq!(buf.as_str(), "select * from orders;");
    }
}
