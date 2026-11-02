//! Logic shared by client and server: cryptography, record format, sync.
//! It never touches the browser and is tested natively.
#![forbid(unsafe_code)]

pub mod analytics;
pub mod api;
pub mod crypto;
pub mod date;
pub mod export;
pub mod model;
pub mod money;
pub mod record;
pub mod sync;
