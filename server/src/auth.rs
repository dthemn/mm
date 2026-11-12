use crate::{State, error::{Error, Result}};
use axum::{
    Json,
    extract::{FromRef, FromRequestParts},
    http::{HeaderValue, header, request::Parts},
    response::{AppendHeaders, IntoResponse},
};
use mm_core::{api::*, crypto::{phrase_challenge, verify_phrase_signature}};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::sync::Arc;
use uuid::Uuid;
use webauthn_rs::prelude::{DiscoverableAuthentication, Passkey, PasskeyRegistration, PublicKeyCredential, RegisterPublicKeyCredential};

const COOKIE: &str = "mm_session";
const SESSION_DAYS: i32 = 30;
const CHALLENGE_MINUTES: i32 = 5;
const WRAP_MAX: usize = 256;
const PURPOSE_LOGIN: &str = "login";
const PURPOSE_ADD: &str = "add-passkey";

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("no randomness source");
    b
}

fn check_wrap(b: &B64) -> Result<()> {
    if b.0.is_empty() || b.0.len() > WRAP_MAX {
        return Err(Error::BadRequest("wrapped key"));
    }
    Ok(())
}

async fn put_challenge<S: Serialize>(app: &crate::App, kind: &str, user: Option<Uuid>, state: Option<&S>, nonce: Option<&[u8]>) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let state = state.map(|s| serde_json::to_value(s).map_err(|_| Error::Internal)).transpose()?;
    sqlx::query("INSERT INTO challenges (id, kind, user_id, state, nonce, expires_at) VALUES ($1,$2,$3,$4,$5, now() + make_interval(mins => $6))")
        .bind(id).bind(kind).bind(user).bind(state).bind(nonce).bind(CHALLENGE_MINUTES)
        .execute(&app.db).await?;
    Ok(id)
}

struct Challenge {
    user_id: Option<Uuid>,
    state: Option<Value>,
    nonce: Option<Vec<u8>>,
}

/// A challenge is single-use: it is deleted atomically on first use.
async fn take_challenge(app: &crate::App, id: Uuid, kind: &str) -> Result<Challenge> {
    let row = sqlx::query("DELETE FROM challenges WHERE id = $1 AND kind = $2 AND expires_at > now() RETURNING user_id, state, nonce")
        .bind(id).bind(kind).fetch_optional(&app.db).await?
        .ok_or(Error::BadRequest("challenge"))?;
    Ok(Challenge { user_id: row.get(0), state: row.get(1), nonce: row.get(2) })
}

fn state_of<T: DeserializeOwned>(c: &Challenge) -> Result<T> {
    serde_json::from_value(c.state.clone().ok_or(Error::Internal)?).map_err(|_| Error::Internal)
}

fn json_of<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| Error::BadRequest("credential"))
}

fn to_value<T: Serialize>(v: &T) -> Result<Value> {
    serde_json::to_value(v).map_err(|_| Error::Internal)
}

fn cookie(app: &crate::App, token: &str, max_age: i64) -> Result<HeaderValue> {
    let secure = if app.secure_cookie { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Path=/api; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}"
    )).map_err(|_| Error::Internal)
}

async fn new_session(app: &crate::App, user: Uuid) -> Result<HeaderValue> {
    let token = hex::encode(random(32));
    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1,$2, now() + make_interval(days => $3))")
        .bind(Sha256::digest(token.as_bytes()).to_vec()).bind(user).bind(SESSION_DAYS)
        .execute(&app.db).await?;
    seen(&app.db, user).await?;
    cookie(app, &token, SESSION_DAYS as i64 * 86400)
}

/// Marks the account as active this minute. Writes at most once a minute per account.
async fn seen(db: &sqlx::PgPool, user: Uuid) -> Result<()> {
    sqlx::query("UPDATE users SET seen_at = date_trunc('minute', now()) WHERE id = $1 AND seen_at IS DISTINCT FROM date_trunc('minute', now())")
        .bind(user).execute(db).await?;
    Ok(())
}

/// The authenticated user taken from the cookie.
pub struct AuthUser(pub Uuid);

fn token_of(parts: &Parts) -> Option<String> {
    parts.headers.get_all(header::COOKIE).iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|c| c.trim().strip_prefix(&format!("{COOKIE}=")).map(str::to_owned))
}

impl<S> FromRequestParts<S> for AuthUser
where
    Arc<crate::App>: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Error;
    async fn from_request_parts(parts: &mut Parts, s: &S) -> Result<Self> {
        let app = Arc::<crate::App>::from_ref(s);
        let token = token_of(parts).ok_or(Error::Unauthorized)?;
        let row = sqlx::query("SELECT user_id FROM sessions WHERE token_hash = $1 AND expires_at > now()")
            .bind(Sha256::digest(token.as_bytes()).to_vec())
            .fetch_optional(&app.db).await?
            .ok_or(Error::Unauthorized)?;
        let user: Uuid = row.get(0);
        seen(&app.db, user).await?;
        Ok(Self(user))
    }
}

/// Registration is open to anyone; it is rate-limited like login.
pub async fn register_start(s: State, Json(q): Json<RegisterStartReq>) -> Result<Json<ChallengeResp>> {
    let label: String = q.label.trim().chars().take(40).collect();
    let label = if label.is_empty() { "mm".to_string() } else { label };
    let user = Uuid::new_v4();
    let (ccr, st) = s.webauthn.start_passkey_registration(user, &label, &label, None)
        .map_err(|_| Error::Internal)?;
    let id = put_challenge::<PasskeyRegistration>(&s, "register", Some(user), Some(&st), None).await?;
    Ok(Json(ChallengeResp { challenge_id: id, user_id: Some(user), options: to_value(&ccr)?, nonce: None }))
}

pub async fn register_finish(s: State, Json(q): Json<RegisterFinishReq>) -> Result<impl IntoResponse> {
    check_wrap(&q.wrapped_dek_phrase)?;
    if let Some(w) = &q.wrapped_dek_prf { check_wrap(w)?; }
    let c = take_challenge(&s, q.challenge_id, "register").await?;
    let user = c.user_id.ok_or(Error::Internal)?;
    let st: PasskeyRegistration = state_of(&c)?;
    let reg: RegisterPublicKeyCredential = json_of(q.credential)?;
    let pk: Passkey = s.webauthn.finish_passkey_registration(&reg, &st)
        .map_err(|_| Error::BadRequest("credential"))?;

    let mut tx = s.db.begin().await?;
    sqlx::query("INSERT INTO users (id, phrase_pub, wrapped_dek_phrase) VALUES ($1,$2,$3)")
        .bind(user).bind(&q.phrase_pub.0).bind(&q.wrapped_dek_phrase.0).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO credentials (cred_id, user_id, passkey, wrapped_dek_prf) VALUES ($1,$2,$3,$4)")
        .bind(AsRef::<[u8]>::as_ref(pk.cred_id())).bind(user).bind(to_value(&pk)?).bind(q.wrapped_dek_prf.as_ref().map(|b| &b.0))
        .execute(&mut *tx).await?;
    tx.commit().await?;

    let set = new_session(&s, user).await?;
    Ok((AppendHeaders([(header::SET_COOKIE, set)]), Json(SessionResp {
        user_id: user, wrapped_dek_prf: q.wrapped_dek_prf, wrapped_dek_phrase: None,
    })))
}

pub async fn login_start(s: State) -> Result<Json<ChallengeResp>> {
    let (rcr, st) = s.webauthn.start_discoverable_authentication().map_err(|_| Error::Internal)?;
    let id = put_challenge::<DiscoverableAuthentication>(&s, "login", None, Some(&st), None).await?;
    let mut options = to_value(&rcr)?;
    // The library forces conditional UI, but login here starts from a button.
    if let Some(o) = options.as_object_mut() { o.remove("mediation"); }
    Ok(Json(ChallengeResp { challenge_id: id, user_id: None, options, nonce: None }))
}

pub async fn login_finish(s: State, Json(q): Json<LoginFinishReq>) -> Result<impl IntoResponse> {
    let c = take_challenge(&s, q.challenge_id, "login").await?;
    let st: DiscoverableAuthentication = state_of(&c)?;
    let cred: PublicKeyCredential = json_of(q.credential)?;
    let (user, cred_id) = s.webauthn.identify_discoverable_authentication(&cred)
        .map_err(|_| Error::Unauthorized)?;
    let row = sqlx::query("SELECT passkey, wrapped_dek_prf FROM credentials WHERE cred_id = $1 AND user_id = $2")
        .bind(cred_id).bind(user).fetch_optional(&s.db).await?
        .ok_or(Error::Unauthorized)?;
    let mut pk: Passkey = serde_json::from_value(row.get(0)).map_err(|_| Error::Internal)?;
    let res = s.webauthn.finish_discoverable_authentication_v2(&cred, st, &pk)
        .map_err(|_| Error::Unauthorized)?;
    if pk.update_credential(&res) == Some(true) {
        sqlx::query("UPDATE credentials SET passkey = $1 WHERE cred_id = $2")
            .bind(to_value(&pk)?).bind(AsRef::<[u8]>::as_ref(pk.cred_id())).execute(&s.db).await?;
    }
    let wrapped: Option<Vec<u8>> = row.get(1);
    let set = new_session(&s, user).await?;
    Ok((AppendHeaders([(header::SET_COOKIE, set)]), Json(SessionResp {
        user_id: user, wrapped_dek_prf: wrapped.map(B64), wrapped_dek_phrase: None,
    })))
}

/// The response does not depend on the key existing: accounts cannot be enumerated.
pub async fn recover_start(s: State, Json(q): Json<RecoverStartReq>) -> Result<Json<RecoverStartResp>> {
    let user: Option<Uuid> = sqlx::query("SELECT id FROM users WHERE phrase_pub = $1")
        .bind(&q.phrase_pub.0).fetch_optional(&s.db).await?.map(|r| r.get(0));
    let nonce = random(32);
    let id = put_challenge::<()>(&s, "recover", user, None, Some(&nonce)).await?;
    Ok(Json(RecoverStartResp { challenge_id: id, nonce: B64(nonce) }))
}

pub async fn recover_finish(s: State, Json(q): Json<RecoverFinishReq>) -> Result<impl IntoResponse> {
    let c = take_challenge(&s, q.challenge_id, "recover").await?;
    let user = c.user_id.ok_or(Error::Unauthorized)?;
    let row = sqlx::query("SELECT phrase_pub, wrapped_dek_phrase FROM users WHERE id = $1")
        .bind(user).fetch_one(&s.db).await?;
    let public: Vec<u8> = row.get(0);
    let msg = phrase_challenge(PURPOSE_LOGIN, q.challenge_id, &c.nonce.unwrap_or_default());
    verify_phrase_signature(&public, &msg, &q.signature.0).map_err(|_| Error::Unauthorized)?;
    let set = new_session(&s, user).await?;
    Ok((AppendHeaders([(header::SET_COOKIE, set)]), Json(SessionResp {
        user_id: user, wrapped_dek_prf: None, wrapped_dek_phrase: Some(B64(row.get(1))),
    })))
}

pub async fn session(AuthUser(user): AuthUser) -> Json<Value> {
    Json(json!({ "user_id": user }))
}

pub async fn logout(s: State, parts: axum::http::HeaderMap) -> Result<impl IntoResponse> {
    let mut p = axum::http::Request::new(()).into_parts().0;
    p.headers = parts;
    if let Some(t) = token_of(&p) {
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(Sha256::digest(t.as_bytes()).to_vec()).execute(&s.db).await?;
    }
    Ok(AppendHeaders([(header::SET_COOKIE, cookie(&s, "", 0)?)]))
}

pub async fn add_passkey_start(s: State, AuthUser(user): AuthUser) -> Result<Json<ChallengeResp>> {
    let rows = sqlx::query("SELECT passkey FROM credentials WHERE user_id = $1")
        .bind(user).fetch_all(&s.db).await?;
    let exclude = rows.iter()
        .map(|r| serde_json::from_value::<Passkey>(r.get(0)).map(|p| p.cred_id().clone()))
        .collect::<std::result::Result<Vec<_>, _>>().map_err(|_| Error::Internal)?;
    let (ccr, st) = s.webauthn.start_passkey_registration(user, "mm", "mm", Some(exclude))
        .map_err(|_| Error::Internal)?;
    let nonce = random(32);
    let id = put_challenge::<PasskeyRegistration>(&s, "add-passkey", Some(user), Some(&st), Some(&nonce)).await?;
    Ok(Json(ChallengeResp { challenge_id: id, user_id: Some(user), options: to_value(&ccr)?, nonce: Some(B64(nonce)) }))
}

pub async fn add_passkey_finish(s: State, AuthUser(user): AuthUser, Json(q): Json<PasskeyAddFinishReq>) -> Result<Json<Value>> {
    if let Some(w) = &q.wrapped_dek_prf { check_wrap(w)?; }
    let c = take_challenge(&s, q.challenge_id, "add-passkey").await?;
    if c.user_id != Some(user) {
        return Err(Error::Unauthorized);
    }
    let public: Vec<u8> = sqlx::query("SELECT phrase_pub FROM users WHERE id = $1")
        .bind(user).fetch_one(&s.db).await?.get(0);
    let nonce = c.nonce.clone().unwrap_or_default();
    // A stolen cookie is not enough: knowledge of the 12 words must be proven.
    verify_phrase_signature(&public, &phrase_challenge(PURPOSE_ADD, q.challenge_id, &nonce), &q.signature.0)
        .map_err(|_| Error::Unauthorized)?;
    let st: PasskeyRegistration = state_of(&c)?;
    let reg: RegisterPublicKeyCredential = json_of(q.credential)?;
    let pk: Passkey = s.webauthn.finish_passkey_registration(&reg, &st)
        .map_err(|_| Error::BadRequest("credential"))?;
    sqlx::query("INSERT INTO credentials (cred_id, user_id, passkey, wrapped_dek_prf) VALUES ($1,$2,$3,$4)")
        .bind(AsRef::<[u8]>::as_ref(pk.cred_id())).bind(user).bind(to_value(&pk)?).bind(q.wrapped_dek_prf.as_ref().map(|b| &b.0))
        .execute(&s.db).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Stores the PRF copy of the DEK for one of the user's passkeys that has none yet. `saved` is false when
/// the passkey is not the user's or already has a copy: an existing copy is never replaced.
pub async fn set_passkey_prf(s: State, AuthUser(user): AuthUser, Json(q): Json<PasskeyPrfReq>) -> Result<Json<Value>> {
    check_wrap(&q.wrapped_dek_prf)?;
    let r = sqlx::query("UPDATE credentials SET wrapped_dek_prf = $1 WHERE cred_id = $2 AND user_id = $3 AND wrapped_dek_prf IS NULL")
        .bind(&q.wrapped_dek_prf.0).bind(&q.cred_id.0).bind(user).execute(&s.db).await?;
    Ok(Json(json!({ "saved": r.rows_affected() == 1 })))
}

pub async fn delete_account(s: State, AuthUser(user): AuthUser) -> Result<impl IntoResponse> {
    sqlx::query("DELETE FROM users WHERE id = $1").bind(user).execute(&s.db).await?;
    Ok(AppendHeaders([(header::SET_COOKIE, cookie(&s, "", 0)?)]))
}
