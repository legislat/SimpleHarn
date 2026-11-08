//! SimpleHarn — one application managing local inference engines with a simple,
//! reliable, adjustable swap mechanism for local LLM models (DESIGN.md G1).
//!
//! Crate layout follows DESIGN.md section 18, and all of it ships: `config` plus
//! `model`/`catalog` read the files that are king, `witness`/`budget`/`scheduler`
//! are the single-writer admission loop, `engine`/`process`/`custody` launch and
//! own the children, `listeners`/`api`/`webui` serve the category endpoints and
//! the control page, `import` is the strata migration bridge, and `tray`
//! (behind the optional `tray` feature), `observability` and `ownership` carry
//! the icon, the logs and the claim ledger. `platform` is the OS seam: the
//! process-identity, process-tree, and single-instance traits behind which the
//! platform implementations live.

pub mod api;
pub mod budget;
pub mod catalog;
pub mod config;
pub mod custody;
pub mod engine;
pub mod import;
pub mod listeners;
pub mod model;
pub mod observability;
pub mod ownership;
pub mod platform;
pub mod prefetch;
pub mod process;
pub mod scheduler;
pub mod testutil;
#[cfg(feature = "tray")]
pub mod tray;
// A binary built without the `tray` feature has no tray module; the
// capture-subscriber path the API test pins drive stays reachable under the
// same name, so the headless build compiles and tests the same suites.
#[cfg(all(test, not(feature = "tray")))]
pub(crate) mod tray {
    pub(crate) use crate::testutil::testlog;
}
pub mod webui;
pub mod witness;
