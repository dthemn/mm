//! Data of this device. The normal mode does not store the DEK at all.
//! Only a device without PRF keeps a DEK copy under a non-extractable WebCrypto key:
//! the 12 words are entered once there, and later logins use the passkey.

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
    kv.del(&format!("devkey:{user}")).await
}
