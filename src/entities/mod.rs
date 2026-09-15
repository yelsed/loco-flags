//! The two tables this crate owns.
//!
//! Public because an application that wants to build a screen on top of these rows should not have
//! to reimplement them, and because the tasks in this crate are only one caller of many.

pub mod feature_flag_overrides;
pub mod feature_flags;
