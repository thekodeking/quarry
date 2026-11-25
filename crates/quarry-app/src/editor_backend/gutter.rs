// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Cached statement index and viewport-only gutter run-button resolution.

use super::buffer::Buffer;
use std::ops::Range;

/// Caches statement start offsets per [`Buffer::version`] so scrolling never re-parses SQL.
#[derive(Debug, Default)]
pub struct StatementIndex {
    version: u64,
    /// `(statement_start_offset, buffer_row)` sorted by offset.
    entries: Vec<(usize, usize)>,
}

impl StatementIndex {
    /// Recomputes statement starts only when `buffer.version()` changes.
    pub fn ensure_updated(&mut self, buffer: &Buffer, rules: quarry_sql::SplitRules) {
        if self.version == buffer.version() && !self.entries.is_empty() {
            return;
        }
        self.version = buffer.version();
        self.entries.clear();
        let sql = buffer.as_str();
        if sql.len() > 256_000 {
            return;
        }
        let spans = if rules.go_batches || rules.slash_blocks {
            quarry_sql::split_statements_with(sql, rules)
        } else {
            quarry_sql::tree_sitter_split_statements(sql)
        };
        self.entries = spans
            .into_iter()
            .map(|s| (s.start, buffer.offset_to_point(s.start).row))
            .collect();
    }

    /// Returns only the `(statement_ix, buffer_row, byte_offset)` entries inside `visible_rows`.
    pub fn visible_entries(&self, visible_rows: Range<usize>) -> &[(usize, usize)] {
        let start = self
            .entries
            .partition_point(|&(_, row)| row < visible_rows.start);
        let end = self
            .entries
            .partition_point(|&(_, row)| row < visible_rows.end);
        &self.entries[start..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_entries_filters_by_row_range() {
        let buf = Buffer::new("select 1;\nselect 2;\nselect 3;\nselect 4;");
        let mut index = StatementIndex::default();
        index.ensure_updated(&buf, quarry_sql::SplitRules::default());
        assert_eq!(index.visible_entries(1..3).len(), 2);
    }
}
