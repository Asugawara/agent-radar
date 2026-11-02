//! Link checking: HTTP classification, state machine and applying results
//! to the tool files.

pub mod http;
pub mod run;
pub mod state;

pub use run::{Options, Report, check_links, run_cli};
