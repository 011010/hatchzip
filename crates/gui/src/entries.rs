//! The contents list. An archive can hold a million entries; the list view
//! only asks for the rows on screen, so rows are built on demand from one
//! plain vector instead of a model per row.

use hatchzip_core::archive::Kind;
use hatchzip_core::engine::Job;
use hatchzip_core::human;
use slint::{Model, ModelTracker};

use crate::EntryRow;

pub struct Entry {
    path: Box<str>,
    size: u64,
    kind: Kind,
}

/// Entries in archive order, as `hatchzip list` prints them.
pub fn collect(job: &Job) -> Vec<Entry> {
    job.units
        .iter()
        .rev()
        .flat_map(|u| &u.items)
        .map(|item| Entry {
            path: item.path.to_string_lossy().into(),
            size: item.size,
            kind: item.kind.clone(),
        })
        .collect()
}

pub struct Entries(pub Vec<Entry>);

impl Model for Entries {
    type Data = EntryRow;

    fn row_count(&self) -> usize {
        self.0.len()
    }

    fn row_data(&self, row: usize) -> Option<EntryRow> {
        let e = self.0.get(row)?;
        let size = match &e.kind {
            Kind::File => human(e.size),
            Kind::Dir => String::new(),
            Kind::Symlink | Kind::HardLink(_) => "link".to_string(),
        };
        Some(EntryRow {
            name: (*e.path).into(),
            size: size.into(),
            dir: e.kind == Kind::Dir,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        // The contents never change; a new archive gets a new model.
        &()
    }
}
