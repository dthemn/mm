//! Login flows. The UI only shows their results.

use crate::api::{self, ApiFail};
use crate::{device, store::{Kv, Store}, webauthn};
use mm_core::api::*;
use mm_core::crypto::{Dek, Phrase, WrapSlot, kek_from_prf, phrase_challenge, unwrap_dek, wrap_dek};
use std::rc::Rc;
use uuid::Uuid;

#[derive(Clone)]
pub struct Session {
    pub user: Uuid,
    pub dek: Rc<Dek>,
    pub store: Rc<Store>,
    /// PRF works for this login. Shown in settings.
    pub prf: bool,
}

pub enum LoginResult {
    Unlocked(Session),
    /// The passkey is verified, but this device has no PRF and is not linked yet.
    NeedPhrase,
}

fn s<T: std::fmt::Display>(x: T) -> String {
    x.to_string()
}

async fn session(user: Uuid, dek: Dek, prf: bool) -> Result<Session, String> {
    let store = Store::open(user).await?;
    Ok(Session { user, dek: Rc::new(dek), store: Rc::new(store), prf })
}

/// PRF output for a new passkey. Some platforms return it only on a second request.
async fn prf_after_create(c: &webauthn::Created) -> Result<Option<Vec<u8>>, String> {
    if c.prf_output.is_some() || !c.prf_enabled {
        return Ok(c.prf_output.clone());
    }
    Ok(webauthn::get_assertion(None, Some(&c.raw_id)).await?.prf_output)
}

pub const BAD_INVITE: &str = "This invite is wrong, already used or expired";

pub async fn check_invite(invite: &str) -> Result<(), String> {
    api::post_only("/api/auth/invite/check", &InviteCheckReq { invite: invite.trim().into() }).await.map_err(|e| match e {
        ApiFail::Status(400, _) => BAD_INVITE.to_string(),
        e => s(e),
    })
}

pub async fn register(invite: &str, label: &str, phrase: &Phrase) -> Result<Session, String> {
    let start: ChallengeResp = api::post("/api/auth/register/start", &RegisterStartReq {
        invite: invite.trim().into(),
        label: label.into(),
    }).await.map_err(|e| match e {
        ApiFail::Status(400, _) => BAD_INVITE.to_string(),
        e => s(e),
    })?;
    let user = start.user_id.ok_or("missing identifier")?;
    let dek = Dek::generate().map_err(s)?;
    let wrapped_dek_phrase = B64(wrap_dek(&phrase.kek(), &dek, user, WrapSlot::Phrase).map_err(s)?);

    let created = webauthn::create(&start.options).await?;
    let prf = prf_after_create(&created).await?;
    let wrapped_dek_prf = prf.as_deref()
        .map(|o| wrap_dek(&kek_from_prf(o).map_err(s)?, &dek, user, WrapSlot::Prf).map_err(s))
        .transpose()?;
    let _: SessionResp = api::post("/api/auth/register/finish", &RegisterFinishReq {
        challenge_id: start.challenge_id,
        invite: invite.trim().into(),
        credential: created.credential.clone(),
        wrapped_dek_prf: wrapped_dek_prf.clone().map(B64),
        wrapped_dek_phrase,
        phrase_pub: B64(phrase.auth_key().public()),
    }).await.map_err(s)?;

    let kv = Kv::open().await?;
    device::add_user(&kv, user).await?;
    match &wrapped_dek_prf {
        Some(w) => device::cache_wrap(&kv, user, &created.raw_id, w).await?,
        // The phrase was just confirmed, so this device need not ask for it again.
        None => device::store_dek(&kv, user, &dek).await?,
    }
    session(user, dek, prf.is_some()).await
}

pub async fn login(online: bool) -> Result<LoginResult, String> {
    let kv = Kv::open().await?;
    let mut server_wrap = None;
    let mut have_session = false;

    let a = 'a: {
        if online {
            match api::post_empty::<ChallengeResp>("/api/auth/login/start").await {
                Ok(c) => {
                    let a = webauthn::get_assertion(Some(&c.options), None).await?;
                    let r: Result<SessionResp, _> = api::post("/api/auth/login/finish", &LoginFinishReq {
                        challenge_id: c.challenge_id,
                        credential: a.credential.clone(),
                    }).await;
                    match r {
                        Ok(r) => {
                            server_wrap = r.wrapped_dek_prf.map(|b| b.0);
                            have_session = true;
                            break 'a a;
                        }
                        Err(ApiFail::Network) => break 'a a,
                        Err(ApiFail::Status(401, _)) => return Err("This passkey is not known to the server".into()),
                        Err(e) => return Err(s(e)),
                    }
                }
                Err(ApiFail::Network) => {}
                Err(e) => return Err(s(e)),
            }
        }
        webauthn::get_assertion(None, None).await?
    };

    let user = Uuid::from_slice(&a.user_handle).map_err(|_| "passkey has no identifier".to_string())?;
    if let Some(w) = &server_wrap {
        device::cache_wrap(&kv, user, &a.raw_id, w).await?;
    }
    device::add_user(&kv, user).await?;
    let wrap = match server_wrap {
        Some(w) => Some(w),
        None => device::cached_wrap(&kv, user, &a.raw_id).await?,
    };
    if let (Some(out), Some(w)) = (&a.prf_output, &wrap) {
        let dek = unwrap_dek(&kek_from_prf(out).map_err(s)?, w, user, WrapSlot::Prf)
            .map_err(|_| "This passkey could not decrypt the key".to_string())?;
        return Ok(LoginResult::Unlocked(session(user, dek, true).await?));
    }
    if let Some(dek) = device::load_dek(&kv, user).await? {
        return Ok(LoginResult::Unlocked(session(user, dek, false).await?));
    }
    if !have_session {
        return Err("The first login on this device needs a connection".into());
    }
    Ok(LoginResult::NeedPhrase)
}

/// Login with the 12 words. `remember` keeps the key on a device without PRF.
pub async fn recover(phrase: &Phrase, remember: bool) -> Result<Session, String> {
    let key = phrase.auth_key();
    let st: RecoverStartResp = api::post("/api/auth/recover/start", &RecoverStartReq { phrase_pub: B64(key.public()) })
        .await.map_err(s)?;
    let sig = key.sign(&phrase_challenge("login", st.challenge_id, &st.nonce.0));
    let r: SessionResp = api::post("/api/auth/recover/finish", &RecoverFinishReq {
        challenge_id: st.challenge_id,
        signature: B64(sig),
    }).await.map_err(|e| match e {
        ApiFail::Status(401, _) => "This phrase does not match any account".to_string(),
        e => s(e),
    })?;
    let wrapped = r.wrapped_dek_phrase.ok_or("the server returned no key")?;
    let dek = unwrap_dek(&phrase.kek(), &wrapped.0, r.user_id, WrapSlot::Phrase)
        .map_err(|_| "Could not decrypt the key".to_string())?;
    let kv = Kv::open().await?;
    device::add_user(&kv, r.user_id).await?;
    if remember {
        device::store_dek(&kv, r.user_id, &dek).await?;
    }
    session(r.user_id, dek, false).await
}

pub async fn add_passkey(sess: &Session, phrase: &Phrase) -> Result<bool, String> {
    let st: ChallengeResp = api::post_empty("/api/passkeys/add/start").await.map_err(s)?;
    let nonce = st.nonce.ok_or("missing challenge value")?;
    let created = webauthn::create(&st.options).await?;
    let prf = prf_after_create(&created).await?;
    let wrapped = prf.as_deref()
        .map(|o| wrap_dek(&kek_from_prf(o).map_err(s)?, &sess.dek, sess.user, WrapSlot::Prf).map_err(s))
        .transpose()?;
    let signature = B64(phrase.auth_key().sign(&phrase_challenge("add-passkey", st.challenge_id, &nonce.0)));
    let _: serde_json::Value = api::post("/api/passkeys/add/finish", &PasskeyAddFinishReq {
        challenge_id: st.challenge_id,
        credential: created.credential.clone(),
        wrapped_dek_prf: wrapped.clone().map(B64),
        signature,
    }).await.map_err(|e| match e {
        ApiFail::Status(401, _) => "This phrase does not match this account".to_string(),
        e => s(e),
    })?;
    let kv = Kv::open().await?;
    match &wrapped {
        Some(w) => device::cache_wrap(&kv, sess.user, &created.raw_id, w).await?,
        None => device::store_dek(&kv, sess.user, &sess.dek).await?,
    }
    Ok(wrapped.is_some())
}

/// A session kept on this device from an earlier sign-in, while it has not expired.
pub async fn resume(now: f64) -> Result<Option<(Session, f64)>, String> {
    let kv = Kv::open().await?;
    for user in device::users(&kv).await? {
        if let Some((dek, prf, until)) = device::kept(&kv, user, now).await? {
            return Ok(Some((session(user, dek, prf).await?, until)));
        }
    }
    Ok(None)
}

pub async fn keep(sess: &Session, until: Option<f64>) -> Result<(), String> {
    let kv = Kv::open().await?;
    match until {
        Some(u) => device::keep(&kv, sess.user, &sess.dek, sess.prf, u).await,
        None => device::forget_kept(&kv, sess.user).await,
    }
}

pub async fn forget_kept() -> Result<(), String> {
    let kv = Kv::open().await?;
    for user in device::users(&kv).await? {
        device::forget_kept(&kv, user).await?;
    }
    Ok(())
}

pub async fn wipe_local(sess: &Session) -> Result<(), String> {
    sess.store.close().await?;
    let kv = Kv::open().await?;
    device::forget_user(&kv, sess.user).await?;
    Store::destroy(sess.user).await
}

pub async fn delete_account(sess: &Session) -> Result<(), String> {
    api::post_done("/api/account/delete").await.map_err(s)?;
    wipe_local(sess).await
}
