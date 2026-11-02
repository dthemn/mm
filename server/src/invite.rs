use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub fn hash(code: &str) -> Vec<u8> {
    Sha256::digest(code.trim().as_bytes()).to_vec()
}

/// Only the hash is stored: a code cannot be taken from a backup.
pub async fn create(db: &PgPool, days: i32) -> Result<String, sqlx::Error> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("no randomness source");
    let code = hex::encode(b);
    sqlx::query("INSERT INTO invites (code_hash, expires_at) VALUES ($1, now() + make_interval(days => $2))")
        .bind(hash(&code))
        .bind(days)
        .execute(db)
        .await?;
    Ok(code)
}
