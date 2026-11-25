// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Lazy viewport-only `DisplayMap` for soft-wrapping and virtualized rendering.
//!
//! Inspired by Zed's `display_map` crate (<https://github.com/zed-industries/zed>, GPL-3.0-or-later).

use super::buffer::Buffer;
use std::ops::Range;

/// A single visual row rendered in the viewport, backed by a slice of a [`Buffer`] line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRow<'a> {
    /// 0-based buffer line index.
    pub buffer_row: usize,
    /// True if this is a wrapped continuation of `buffer_row`.
    pub is_continuation: bool,
    /// Slice of text for this visual row.
    pub text: &'a str,
}

/// Computes visible display rows lazily for the current scroll viewport.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayMap {
    pub wrap_columns: Option<usize>,
    pub overscan_rows: usize,
}

impl DisplayMap {
    pub fn new(wrap_columns: Option<usize>) -> Self {
        Self {
            wrap_columns: wrap_columns.filter(|&c| c > 0),
            overscan_rows: 4,
        }
    }

    /// Returns the buffer line range `[start, end)` visible in the viewport plus overscan.
    pub fn visible_buffer_range(
        &self,
        scroll_y: f32,
        viewport_height: f32,
        line_height: f32,
        total_lines: usize,
    ) -> Range<usize> {
        if total_lines == 0 || line_height <= 0.0 || viewport_height <= 0.0 {
            return 0..0;
        }
        let first = (scroll_y.max(0.0) / line_height).floor() as usize;
        let visible_count = (viewport_height / line_height).ceil() as usize + 1;
        let start = first.saturating_sub(self.overscan_rows);
        let end = (first + visible_count + self.overscan_rows).min(total_lines);
        start..end
    }

    /// Builds [`DisplayRow`]s only for `buffer_rows`, avoiding off-screen layout work.
    pub fn rows_for_range<'a>(
        &self,
        buffer: &'a Buffer,
        buffer_rows: Range<usize>,
    ) -> Vec<DisplayRow<'a>> {
        let mut out = Vec::with_capacity(buffer_rows.len());
        let max_cols = self.wrap_columns;
        for row in buffer_rows {
            let Some(line) = buffer.line(row) else {
                continue;
            };
            match max_cols {
                None => out.push(DisplayRow {
                    buffer_row: row,
                    is_continuation: false,
                    text: line,
                }),
                Some(cols) if line.chars().count() <= cols => out.push(DisplayRow {
                    buffer_row: row,
                    is_continuation: false,
                    text: line,
                }),
                Some(cols) => {
                    let mut start = 0usize;
                    let mut count = 0usize;
                    let mut continuation = false;
                    for (idx, _) in line.char_indices() {
                        if count == cols {
                            out.push(DisplayRow {
                                buffer_row: row,
                                is_continuation: continuation,
                                text: &line[start..idx],
                            });
                            start = idx;
                            count = 0;
                            continuation = true;
                        }
                        count += 1;
                    }
                    out.push(DisplayRow {
                        buffer_row: row,
                        is_continuation: continuation,
                        text: &line[start..],
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_clamps_to_buffer() {
        let map = DisplayMap::new(None);
        assert_eq!(map.visible_buffer_range(0.0, 200.0, 20.0, 100), 0..15);
        assert_eq!(map.visible_buffer_range(1900.0, 200.0, 20.0, 100), 91..100);
    }

    #[test]
    fn wraps_only_requested_rows() {
        let buf = Buffer::new("abcdefghij\n123");
        let map = DisplayMap::new(Some(4));
        let rows = map.rows_for_range(&buf, 0..2);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].text, "abcd");
        assert!(!rows[0].is_continuation);
        assert_eq!(rows[1].text, "efgh");
        assert!(rows[1].is_continuation);
        assert_eq!(rows[2].text, "ij");
        assert_eq!(rows[3].text, "123");
    }
}
