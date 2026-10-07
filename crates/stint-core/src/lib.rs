//! stint-core: the time-tracker domain core, ported from `stintcore/`.
//!
//! Layout mirrors the Python package so the two can be read side by side:
//!   * [`config`]  rate, categories, clients, ledger location
//!   * [`engine`]  stop-time billing rules (reboot cap, quarter-hour round-up)
//!   * [`store`]   CSV + `.timer` file I/O, edits, totals
//!   * [`invoice`] consolidation, staging, counters, HTML render
//!   * [`report`]  badges + per-category / per-week rollups
//!
//! Byte-parity with the Bash `stint.sh` script and the Python core is enforced by
//! the repo's pytest suite, which drives this crate through the `stint parity`
//! subcommand.

pub mod config;
pub mod engine;
pub mod invoice;
pub mod report;
pub mod store;

pub use config::Home;
