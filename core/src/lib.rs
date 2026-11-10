//! Logic shared by client and server: cryptography, record format, sync.
//! It never touches the browser and is tested natively.
#![forbid(unsafe_code)]

pub mod analytics;
pub mod api;
pub mod categorize;
pub mod confirmation;
pub mod crypto;
pub mod date;
pub mod dedup;
pub mod export;
pub mod import;
pub mod model;
pub mod money;
pub mod pdf;
pub mod ocr;
pub mod receipt;
pub mod record;
pub mod sync;
pub mod vision;
