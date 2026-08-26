//! Deferred job system (Bottom half).
//! Re-exported from zcore-drivers to avoid circular dependencies.

pub use crate::drivers::utils::deferred_job::{
    drain_deferred_jobs, drain_deferred_jobs_max, pending_deferred_jobs, push_deferred_job,
};
