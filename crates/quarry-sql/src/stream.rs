// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Streaming SQL statement reader for executing large `.sql` files in $O(\text{statement})$ memory.

use crate::splitter::SplitRules;
use std::io::BufRead;

/// Incremental statement iterator over any [`BufRead`] source.
pub struct StatementStream<R: BufRead> {
    reader: R,
    rules: SplitRules,
    buf: String,
    line_buf: String,
    in_single_quote: bool,
    in_double_quote: bool,
    in_block_comment: bool,
    bytes_read: u64,
    eof: bool,
}

impl<R: BufRead> StatementStream<R> {
    pub fn new(reader: R, rules: SplitRules) -> Self {
        Self {
            reader,
            rules,
            buf: String::new(),
            line_buf: String::new(),
            in_single_quote: false,
            in_double_quote: false,
            in_block_comment: false,
            bytes_read: 0,
            eof: false,
        }
    }

    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    fn flush_non_empty(&mut self) -> Option<String> {
        let trimmed = self.buf.trim();
        if trimmed.is_empty() {
            self.buf.clear();
            None
        } else {
            let out = trimmed.to_string();
            self.buf.clear();
            Some(out)
        }
    }
}

impl<R: BufRead> Iterator for StatementStream<R> {
    type Item = Result<String, std::io::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.eof {
                return self.flush_non_empty().map(Ok);
            }
            self.line_buf.clear();
            match self.reader.read_line(&mut self.line_buf) {
                Ok(0) => {
                    self.eof = true;
                    if let Some(stmt) = self.flush_non_empty() {
                        return Some(Ok(stmt));
                    }
                    return None;
                }
                Ok(n) => {
                    self.bytes_read += n as u64;
                }
                Err(e) => return Some(Err(e)),
            }

            let trimmed = self.line_buf.trim();
            if !self.in_single_quote && !self.in_double_quote && !self.in_block_comment {
                if self.rules.slash_blocks && trimmed == "/" {
                    if let Some(stmt) = self.flush_non_empty() {
                        return Some(Ok(stmt));
                    }
                    continue;
                }
                if self.rules.go_batches && trimmed.eq_ignore_ascii_case("GO") {
                    if let Some(stmt) = self.flush_non_empty() {
                        return Some(Ok(stmt));
                    }
                    continue;
                }
            }

            let line = std::mem::take(&mut self.line_buf);
            let bytes = line.as_bytes();
            let mut i = 0;
            let mut last_split = 0;
            let mut emit_stmt: Option<String> = None;

            while i < bytes.len() {
                if self.in_block_comment {
                    if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        self.in_block_comment = false;
                        i += 2;
                        continue;
                    }
                    i += 1;
                    continue;
                }
                if self.in_single_quote {
                    if bytes[i] == b'\'' {
                        if bytes.get(i + 1) == Some(&b'\'') {
                            i += 2;
                            continue;
                        }
                        self.in_single_quote = false;
                    }
                    i += 1;
                    continue;
                }
                if self.in_double_quote {
                    if bytes[i] == b'"' {
                        if bytes.get(i + 1) == Some(&b'"') {
                            i += 2;
                            continue;
                        }
                        self.in_double_quote = false;
                    }
                    i += 1;
                    continue;
                }

                if bytes[i] == b'-' && bytes.get(i + 1) == Some(&b'-') {
                    break;
                }
                if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    self.in_block_comment = true;
                    i += 2;
                    continue;
                }
                if bytes[i] == b'\'' {
                    self.in_single_quote = true;
                    i += 1;
                    continue;
                }
                if bytes[i] == b'"' {
                    self.in_double_quote = true;
                    i += 1;
                    continue;
                }
                if bytes[i] == b';' {
                    self.buf.push_str(&line[last_split..=i]);
                    last_split = i + 1;
                    if let Some(stmt) = self.flush_non_empty() {
                        emit_stmt = Some(stmt);
                        break;
                    }
                }
                i += 1;
            }

            if let Some(stmt) = emit_stmt {
                if last_split < line.len() {
                    self.buf.push_str(&line[last_split..]);
                }
                self.line_buf = line;
                return Some(Ok(stmt));
            }
            self.buf.push_str(&line[last_split..]);
            self.line_buf = line;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn streams_statements_respecting_quotes_and_comments() {
        let sql = "SELECT 'a;b';\n-- comment ;\nINSERT INTO t VALUES (1);\n";
        let stmts: Vec<String> = StatementStream::new(Cursor::new(sql), SplitRules::default())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stmts.len(), 2);
        assert_eq!(stmts[0], "SELECT 'a;b';");
        assert!(stmts[1].contains("INSERT INTO t VALUES (1);"));
    }

    #[test]
    fn streams_oracle_slash_blocks() {
        let sql = "BEGIN\n  NULL;\nEND;\n/\nSELECT 1 FROM DUAL;\n";
        let rules = SplitRules {
            slash_blocks: true,
            ..SplitRules::default()
        };
        let stmts: Vec<String> = StatementStream::new(Cursor::new(sql), rules)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(stmts.len(), 3);
    }
}
