//! Data of this device. The normal mode does not store the DEK at all.
//! Only a device without PRF keeps a DEK copy under a non-extractable WebCrypto key:
//! the 12 words are entered once there, and later logins use the passkey.
//! While the user chose to stay signed in, a second copy lets a reload open without the passkey.

use crate::store::{Kv, js_arr, js_obj};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B};
use js_sys::{Object, Uint8Array};
use mm_core::crypto::Dek;
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CryptoKey, SubtleCrypto};
use zeroize::Zeroizing;

const USERS: &str = "users";

fn e<T: std::fmt::Debug>(x: T) -> String {
    format!("{x:?}")
}

fn subtle() -> Result<SubtleCrypto, String> {
    Ok(web_sys::window().ok_or("no window")?.crypto().map_err(e)?.subtle())
}

fn algo(iv: &[u8]) -> Object {
    js_obj(&[("name", "AES-GCM".into()), ("iv", Uint8Array::from(iv).into())])
}

async fn device_key(kv: &Kv, user: Uuid) -> Result<CryptoKey, String> {
    let name = format!("devkey:{user}");
    if let Some(k) = kv.get(&name).await? {
        return k.dyn_into().map_err(e);
    }
    let params = js_obj(&[("name", "AES-GCM".into()), ("length", 256.into())]);
    let p = subtle()?.generate_key_with_object(&params, false, &js_arr(&["encrypt", "decrypt"])).map_err(e)?;
    let key = JsFuture::from(p).await.map_err(e)?;
    kv.put(&name, &key).await?;
    key.dyn_into().map_err(e)
}

pub async fn store_dek(kv: &Kv, user: Uuid, dek: &Dek) -> Result<(), String> {
    let key = device_key(kv, user).await?;
    let mut iv = [0u8; 12];
    getrandom::fill(&mut iv).map_err(e)?;
    let mut data = *dek.expose();
    let p = subtle()?.encrypt_with_object_and_u8_array(&algo(&iv), &key, &data).map_err(e)?;
    data.fill(0);
    let ct = JsFuture::from(p).await.map_err(e)?;
    let mut out = iv.to_vec();
    out.extend(Uint8Array::new(&ct).to_vec());
    kv.put(&format!("devdek:{user}"), &JsValue::from_str(&B.encode(out))).await
}

pub async fn load_dek(kv: &Kv, user: Uuid) -> Result<Option<Dek>, String> {
    let Some(v) = kv.get(&format!("devdek:{user}")).await? else { return Ok(None) };
    let blob = B.decode(v.as_string().unwrap_or_default()).map_err(e)?;
    if blob.len() < 12 {
        return Err("invalid key copy".into());
    }
    let (iv, ct) = blob.split_at(12);
    let key = device_key(kv, user).await?;
    let p = subtle()?.decrypt_with_object_and_u8_array(&algo(iv), &key, ct).map_err(e)?;
    let pt = JsFuture::from(p).await.map_err(e)?;
    let bytes = Zeroizing::new(Uint8Array::new(&pt).to_vec());
    Dek::from_slice(&bytes).map(Some).map_err(e)
}

pub async fn cache_wrap(kv: &Kv, user: Uuid, cred: &[u8], wrapped: &[u8]) -> Result<(), String> {
    kv.put(&format!("wrap:{user}:{}", B.encode(cred)), &JsValue::from_str(&B.encode(wrapped))).await
}

pub async fn cached_wrap(kv: &Kv, user: Uuid, cred: &[u8]) -> Result<Option<Vec<u8>>, String> {
    let v = kv.get(&format!("wrap:{user}:{}", B.encode(cred))).await?;
    v.map(|v| B.decode(v.as_string().unwrap_or_default()).map_err(e)).transpose()
}

pub async fn users(kv: &Kv) -> Result<Vec<Uuid>, String> {
    let v = kv.get(USERS).await?.and_then(|v| v.as_string()).unwrap_or_default();
    Ok(v.split(',').filter_map(|s| s.parse().ok()).collect())
}

pub async fn add_user(kv: &Kv, user: Uuid) -> Result<(), String> {
    let mut u = users(kv).await?;
    if !u.contains(&user) {
        u.push(user);
        let s = u.iter().map(Uuid::to_string).collect::<Vec<_>>().join(",");
        kv.put(USERS, &JsValue::from_str(&s)).await?;
    }
    Ok(())
}

pub async fn forget_user(kv: &Kv, user: Uuid) -> Result<(), String> {
    let u: Vec<String> = users(kv).await?.iter().filter(|x| **x != user).map(Uuid::to_string).collect();
    kv.put(USERS, &JsValue::from_str(&u.join(","))).await?;
    kv.del(&format!("devdek:{user}")).await?;
    kv.del(&format!("devkey:{user}")).await?;
    forget_kept(kv, user).await
}

/// How long the app stays signed in on this device, in minutes. Kept in localStorage: it is a setting of the device,
/// not of the account, and it has to be read before any key exists.
pub const KEEP_CHOICES: [(u32, &str); 3] = [(1, "1 min"), (60, "1 hour"), (720, "12 hours")];
const KEEP_DEFAULT: u32 = 720;

pub fn keep_minutes() -> u32 {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("mmKeep").ok().flatten())
        .and_then(|v| v.parse().ok())
        .filter(|m| KEEP_CHOICES.iter().any(|c| c.0 == *m))
        .unwrap_or(KEEP_DEFAULT)
}

pub fn set_keep_minutes(m: u32) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item("mmKeep", &m.to_string());
    }
}

/// The end of the kept sign-in, also in localStorage: unlike IndexedDB it is cleared at once when the app locks,
/// so a reload right after locking cannot open with a key copy that was not deleted yet.
pub fn keep_flag() -> Option<f64> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("mmKeepUntil").ok().flatten())
        .and_then(|v| v.parse().ok())
}

pub fn set_keep_flag(until: Option<f64>) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = match until {
            Some(u) => s.set_item("mmKeepUntil", &u.to_string()),
            None => s.remove_item("mmKeepUntil"),
        };
    }
}

/// The key copy that keeps the app signed in across a reload or a page the phone dropped in the background.
/// Under its own non-extractable key, deleted on lock; `until` is checked here, as the device is the only one
/// that can open it anyway.
async fn keep_key(kv: &Kv, user: Uuid) -> Result<CryptoKey, String> {
    let name = format!("keepkey:{user}");
    if let Some(k) = kv.get(&name).await? {
        return k.dyn_into().map_err(e);
    }
    let params = js_obj(&[("name", "AES-GCM".into()), ("length", 256.into())]);
    let p = subtle()?.generate_key_with_object(&params, false, &js_arr(&["encrypt", "decrypt"])).map_err(e)?;
    let key = JsFuture::from(p).await.map_err(e)?;
    kv.put(&name, &key).await?;
    key.dyn_into().map_err(e)
}

pub async fn keep(kv: &Kv, user: Uuid, dek: &Dek, prf: bool, until: f64) -> Result<(), String> {
    let key = keep_key(kv, user).await?;
    let mut iv = [0u8; 12];
    getrandom::fill(&mut iv).map_err(e)?;
    let mut data = *dek.expose();
    let p = subtle()?.encrypt_with_object_and_u8_array(&algo(&iv), &key, &data).map_err(e)?;
    data.fill(0);
    let ct = JsFuture::from(p).await.map_err(e)?;
    let mut out = iv.to_vec();
    out.extend(Uint8Array::new(&ct).to_vec());
    kv.put(&format!("keep:{user}"), &JsValue::from_str(&format!("{until}|{}|{}", u8::from(prf), B.encode(out)))).await
}

/// The kept key of a user, if it has not expired. An expired one is deleted.
pub async fn kept(kv: &Kv, user: Uuid, now: f64) -> Result<Option<(Dek, bool, f64)>, String> {
    let Some(v) = kv.get(&format!("keep:{user}")).await?.and_then(|v| v.as_string()) else { return Ok(None) };
    let mut parts = v.splitn(3, '|');
    let (Some(until), Some(prf), Some(blob)) = (parts.next(), parts.next(), parts.next()) else { return Ok(None) };
    let until: f64 = until.parse().unwrap_or(0.0);
    if until <= now || keep_flag() != Some(until) {
        forget_kept(kv, user).await?;
        return Ok(None);
    }
    let blob = B.decode(blob).map_err(e)?;
    if blob.len() < 12 {
        return Ok(None);
    }
    let (iv, ct) = blob.split_at(12);
    let key = keep_key(kv, user).await?;
    let p = subtle()?.decrypt_with_object_and_u8_array(&algo(iv), &key, ct).map_err(e)?;
    let pt = JsFuture::from(p).await.map_err(e)?;
    let bytes = Zeroizing::new(Uint8Array::new(&pt).to_vec());
    Ok(Some((Dek::from_slice(&bytes).map_err(e)?, prf == "1", until)))
}

pub async fn forget_kept(kv: &Kv, user: Uuid) -> Result<(), String> {
    kv.del(&format!("keep:{user}")).await?;
    kv.del(&format!("keepkey:{user}")).await
}
