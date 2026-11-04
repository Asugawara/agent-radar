//! agent-radar core: data model, parsing (from `&str`), validation and the
//! `SiteData` contract shared with the table UI. No file or network I/O, so
//! this crate also builds for `wasm32`.

pub mod guide;
pub mod icons;
pub mod model;
pub mod query;
pub mod site;
pub mod validate;

pub use model::*;
pub use query::{
    ALL_PLATFORMS, Counts, Group, Query, ReadState, Sort, SortKey, TagMode, merge_query_string,
};
pub use site::*;
pub use validate::{
    Diagnostic, Project, Severity, SourceFile, check_issue_form, has_errors, is_slug, validate,
};
