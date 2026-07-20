use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct MdbSchema {
    pub tables: Vec<String>,
    pub columns: BTreeMap<String, Vec<String>>,
}

impl MdbSchema {
    pub fn has_table(&self, table: &str) -> bool {
        self.tables.iter().any(|candidate| candidate == table)
    }

    pub fn set_columns(&mut self, table: impl Into<String>, columns: Vec<String>) {
        self.columns.insert(table.into(), columns);
    }
}
