//! File › Print is not part of Studio's build: the print spooler and its system print routes
//! were left out when this engine was vendored. The types the session keeps stay, so the rest of
//! the engine is unchanged; no print command is registered.

use crate::commands::CommandSpec;

/// A rendered page on its way to the printer.
#[derive(Debug, Clone)]
pub struct PrintJob {
    pub pdf: Vec<u8>,
    pub printer: Option<String>,
    pub copies: u32,
    pub title: String,
    pub server: Option<String>,
    pub paper: (f64, f64),
    pub landscape: bool,
}

/// A spooler's work: the job in, where it went out.
pub type SpoolFn = std::sync::Arc<dyn Fn(&PrintJob) -> std::result::Result<String, String> + Send + Sync>;

/// A further route to the printer (unused in this build).
#[derive(Clone)]
pub struct Spooler {
    pub name: &'static str,
    pub preferred: bool,
    pub run: SpoolFn,
}

/// Printing is always off in this build.
pub fn disable_printing() {}

/// Printing is always off in this build.
pub fn printing_disabled() -> bool {
    true
}

pub(crate) fn specs() -> Vec<CommandSpec> {
    Vec::new()
}
