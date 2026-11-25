// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Krishna <https://github.com/thekodeking/quarry>

//! Dialect-agnostic DDL generation and `ALTER TABLE` diffing for the visual table designer.

use crate::schema::TableSchema;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSpec {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSpec {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnSpec>,
}

/// Generates a `CREATE TABLE` statement from a [`TableSpec`].
pub fn generate_create_table_sql(spec: &TableSpec) -> String {
    let cols: Vec<String> = spec
        .columns
        .iter()
        .map(|c| {
            let mut parts = vec![format!("\"{}\" {}", c.name, c.data_type)];
            if !c.nullable {
                parts.push("NOT NULL".into());
            }
            if c.primary_key {
                parts.push("PRIMARY KEY".into());
            }
            if let Some(def) = &c.default_value {
                parts.push(format!("DEFAULT {def}"));
            }
            format!("  {}", parts.join(" "))
        })
        .collect();
    let target = if spec.schema.is_empty() {
        format!("\"{}\"", spec.name)
    } else {
        format!("\"{}\".\"{}\"", spec.schema, spec.name)
    };
    format!("CREATE TABLE {target} (\n{}\n);", cols.join(",\n"))
}

/// Computes `ALTER TABLE` statements to migrate `old` to `new`.
pub fn diff_table_sql(old: &TableSchema, new: &TableSpec) -> Vec<String> {
    let target = if old.schema_name.is_empty() {
        format!("\"{}\"", old.table_name)
    } else {
        format!("\"{}\".\"{}\"", old.schema_name, old.table_name)
    };
    let mut stmts = Vec::new();
    for col in &new.columns {
        if !old.columns.iter().any(|c| c.name == col.name) {
            let null_sql = if col.nullable { "" } else { " NOT NULL" };
            stmts.push(format!(
                "ALTER TABLE {target} ADD COLUMN \"{}\" {}{null_sql};",
                col.name, col.data_type
            ));
        }
    }
    for old_col in &old.columns {
        if !new.columns.iter().any(|c| c.name == old_col.name) {
            stmts.push(format!(
                "ALTER TABLE {target} DROP COLUMN \"{}\";",
                old_col.name
            ));
        }
    }
    stmts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generates_create_table_sql() {
        let spec = TableSpec {
            schema: "public".into(),
            name: "users".into(),
            columns: vec![
                ColumnSpec {
                    name: "id".into(),
                    data_type: "integer".into(),
                    nullable: false,
                    primary_key: true,
                    default_value: None,
                },
                ColumnSpec {
                    name: "email".into(),
                    data_type: "text".into(),
                    nullable: false,
                    primary_key: false,
                    default_value: None,
                },
            ],
        };
        let sql = generate_create_table_sql(&spec);
        assert!(sql.contains("CREATE TABLE \"public\".\"users\""));
        assert!(sql.contains("\"id\" integer NOT NULL PRIMARY KEY"));
    }
}
