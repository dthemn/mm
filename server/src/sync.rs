use crate::{State, auth::AuthUser, error::{Error, Result}};
use axum::{Json, extract::Query};
use mm_core::{api::*, crypto::MAX_BUCKET};
use serde::Deserialize;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

const PAGE: i64 = 200;
const MAX_ITEMS: usize = 100;
/// Padding, format byte, nonce and tag.
const MAX_BLOB: usize = MAX_BUCKET + 64;

#[derive(Deserialize)]
pub struct PullQuery {
    since: u64,
}

fn change(r: &sqlx::postgres::PgRow) -> Change {
    Change {
        id: r.get("id"),
        revision: r.get::<i64, _>("revision") as u64,
        seq: r.get::<i64, _>("seq") as u64,
        blob: B64(r.get("blob")),
    }
}

pub async fn pull(s: State, AuthUser(user): AuthUser, Query(q): Query<PullQuery>) -> Result<Json<PullResp>> {
    if q.since > i64::MAX as u64 {
        return Err(Error::BadRequest("since"));
    }
    let rows = sqlx::query("SELECT id, revision, seq, blob FROM records WHERE user_id = $1 AND seq > $2 ORDER BY seq LIMIT $3")
        .bind(user).bind(q.since as i64).bind(PAGE).fetch_all(&s.db).await?;
    let changes: Vec<Change> = rows.iter().map(change).collect();
    Ok(Json(PullResp {
        next: changes.last().map(|c| c.seq).unwrap_or(q.since),
        more: rows.len() as i64 == PAGE,
        changes,
    }))
}

async fn write(tx: &mut Transaction<'_, Postgres>, user: Uuid, seq: &mut i64, item: &PushItem, new_rev: i64) -> Result<()> {
    *seq = seq.checked_add(1).ok_or(Error::BadRequest("seq"))?;
    // Time is rounded to the hour: the server has no need for the exact rhythm of purchases.
    sqlx::query(
        "INSERT INTO records (user_id, id, revision, seq, size, updated_at, blob)
         VALUES ($1,$2,$3,$4,$5, date_trunc('hour', now()), $6)
         ON CONFLICT (user_id, id) DO UPDATE SET
           revision = $3, seq = $4, size = $5, updated_at = date_trunc('hour', now()), blob = $6",
    )
    .bind(user).bind(item.id).bind(new_rev).bind(*seq).bind(item.blob.0.len() as i32).bind(&item.blob.0)
    .execute(&mut **tx).await?;
    Ok(())
}

pub async fn push(s: State, AuthUser(user): AuthUser, Json(q): Json<PushReq>) -> Result<Json<PushResp>> {
    if q.items.len() > MAX_ITEMS {
        return Err(Error::BadRequest("too many items"));
    }
    if q.items.iter().any(|i| i.blob.0.is_empty() || i.blob.0.len() > MAX_BLOB) {
        return Err(Error::BadRequest("record size"));
    }
    // Revisions are stored as BIGINT; a made-up one near u64::MAX would overflow.
    if q.items.iter().any(|i| i.expected_revision >= i64::MAX as u64) {
        return Err(Error::BadRequest("revision"));
    }
    let mut tx = s.db.begin().await?;
    // Locking the user row orders writes: clients see seq numbers
    // strictly ascending, so no change can be missed while reading.
    let row = sqlx::query("SELECT seq, bytes FROM users WHERE id = $1 FOR UPDATE")
        .bind(user).fetch_one(&mut *tx).await?;
    let mut seq: i64 = row.get(0);
    let before: i64 = row.get(1);
    let mut bytes = before;
    let mut results = Vec::with_capacity(q.items.len());
    for item in &q.items {
        let cur = sqlx::query("SELECT id, revision, seq, blob FROM records WHERE user_id = $1 AND id = $2")
            .bind(user).bind(item.id).fetch_optional(&mut *tx).await?;
        let expected = item.expected_revision as i64;
        let res = match cur.as_ref().map(change) {
            // The same write repeated after a lost response.
            Some(c) if c.revision == item.expected_revision + 1 && c.blob == item.blob => {
                PushResult::Ok { id: item.id, revision: c.revision }
            }
            Some(c) if c.revision as i64 != expected => PushResult::Conflict { id: item.id, current: c },
            _ => {
                let old = cur.as_ref().map_or(0, |r| r.get::<Vec<u8>, _>("blob").len() as i64);
                bytes += item.blob.0.len() as i64 - old;
                write(&mut tx, user, &mut seq, item, expected + 1).await?;
                PushResult::Ok { id: item.id, revision: item.expected_revision + 1 }
            }
        };
        results.push(res);
    }
    // Open registration must not let one account fill the disk. A push that shrinks the data always
    // passes, so an account over the quota can still delete. The transaction rolls back on drop.
    if bytes > s.quota_bytes && bytes > before {
        return Err(Error::Quota);
    }
    sqlx::query("UPDATE users SET seq = $1, bytes = $2 WHERE id = $3").bind(seq).bind(bytes).bind(user).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(PushResp { results }))
}
