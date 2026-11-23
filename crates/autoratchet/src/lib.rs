//! AutoRatchet: prevent code quality regressions, one pull request at a time.

pub mod align;
pub mod collect;
pub mod compare;
pub mod config;
pub mod functions;
pub mod gate;
pub mod git;
pub mod go;
pub mod lang;
mod namesakes;
pub mod overrides;
/// The anti-slop rules AutoRatchet runs through Oxlint, vendored by scripts/vendor-anti-slop.mjs.
pub mod plugin {
    include!("../plugin/files.rs");
}
pub mod python;
mod python_cast;
pub mod report;
pub mod rust;
mod settings;
pub mod slop;
pub mod snapshots;
pub mod tools;
