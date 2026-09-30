//! On-disk `.pageindex/` format, byte-compatible with the Python SDK's local mode
//! (`pageindex/local_store.py`, the storage half of `pageindex/local_api.py`, and
//! `pageindex/naming.py`).

pub mod api;
pub mod consts;
pub mod naming;
pub mod pyjson;
pub mod store;

pub use api::{ApiError, ApiResult, LocalApi, NewDocument};
pub use store::{DocStore, Meta, StoreLock};
