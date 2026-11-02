//! WebAuthn in the browser. `webauthn-rs` has no PRF support, so the extension is added here.
//! The PRF output never leaves the device: only signature fields go to the server.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B};
use js_sys::{Array, Function, JSON, Object, Reflect, Uint8Array};
use mm_core::crypto::prf_salt;
use serde_json::{Value, json};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

fn e(v: JsValue) -> String {
    let name = Reflect::get(&v, &"name".into()).ok().and_then(|x| x.as_string()).unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" => "The passkey action was cancelled or not allowed".into(),
        "InvalidStateError" => "This passkey is already registered on this device".into(),
        "NotSupportedError" | "SecurityError" => "This browser does not support passkeys on this page".into(),
        _ => format!("Passkey error: {name}"),
    }
}

fn jerr<T: std::fmt::Debug>(x: T) -> String {
    format!("{x:?}")
}

fn get(o: &JsValue, k: &str) -> JsValue {
    Reflect::get(o, &k.into()).unwrap_or(JsValue::UNDEFINED)
}

fn set(o: &JsValue, k: &str, v: &JsValue) -> Result<(), String> {
    Reflect::set(o, &k.into(), v).map(|_| ()).map_err(e)
}

fn to_buf(b: &[u8]) -> JsValue {
    Uint8Array::from(b).buffer().into()
}

fn bytes(v: &JsValue) -> Vec<u8> {
    if v.is_undefined() || v.is_null() { vec![] } else { Uint8Array::new(v).to_vec() }
}

fn b64(v: &JsValue) -> String {
    B.encode(bytes(v))
}

/// Turns base64url strings from the server into ArrayBuffers for the browser.
fn decode_field(o: &JsValue, k: &str) -> Result<(), String> {
    if let Some(s) = get(o, k).as_string() {
        set(o, k, &to_buf(&B.decode(s).map_err(jerr)?))?;
    }
    Ok(())
}

fn decode_ids(o: &JsValue, k: &str) -> Result<(), String> {
    if let Some(a) = get(o, k).dyn_ref::<Array>() {
        for c in a.iter() {
            decode_field(&c, "id")?;
        }
    }
    Ok(())
}

fn to_js(v: &Value) -> Result<JsValue, String> {
    JSON::parse(&v.to_string()).map_err(e)
}

fn prf_ext(eval: bool) -> Result<JsValue, String> {
    let prf: JsValue = Object::new().into();
    if eval {
        let ev: JsValue = Object::new().into();
        set(&ev, "first", &to_buf(&prf_salt()))?;
        set(&prf, "eval", &ev)?;
    }
    Ok(prf)
}

fn credentials() -> Result<web_sys::CredentialsContainer, String> {
    Ok(web_sys::window().ok_or("no window")?.navigator().credentials())
}

fn prf_output(cred: &JsValue) -> (bool, Option<Vec<u8>>) {
    let results = get(cred, "getClientExtensionResults")
        .dyn_ref::<Function>()
        .and_then(|f| f.call0(cred).ok())
        .unwrap_or(JsValue::UNDEFINED);
    let prf = get(&results, "prf");
    let out = get(&get(&prf, "results"), "first");
    let output = (!out.is_undefined()).then(|| bytes(&out));
    let enabled = get(&prf, "enabled").as_bool().unwrap_or(false) || output.is_some();
    (enabled, output)
}

pub struct Created {
    pub credential: Value,
    pub raw_id: Vec<u8>,
    pub prf_enabled: bool,
    pub prf_output: Option<Vec<u8>>,
}

pub async fn create(options: &Value) -> Result<Created, String> {
    let root = to_js(options)?;
    let pk = get(&root, "publicKey");
    decode_field(&pk, "challenge")?;
    decode_field(&get(&pk, "user"), "id")?;
    decode_ids(&pk, "excludeCredentials")?;
    // The passkey must be discoverable, so login works on a new device without a username.
    let sel: JsValue = Object::new().into();
    set(&sel, "residentKey", &"required".into())?;
    set(&sel, "requireResidentKey", &true.into())?;
    set(&sel, "userVerification", &"required".into())?;
    set(&pk, "authenticatorSelection", &sel)?;
    let ext = get(&pk, "extensions");
    let ext = if ext.is_object() { ext } else { Object::new().into() };
    set(&ext, "prf", &prf_ext(true)?)?;
    set(&pk, "extensions", &ext)?;

    let opts = web_sys::CredentialCreationOptions::from(JsValue::from(Object::new()));
    set(&opts, "publicKey", &pk)?;
    let cred = JsFuture::from(credentials()?.create_with_options(&opts).map_err(e)?).await.map_err(e)?;
    let resp = get(&cred, "response");
    let raw_id = bytes(&get(&cred, "rawId"));
    let (prf_enabled, prf_output) = prf_output(&cred);
    Ok(Created {
        credential: json!({
            "id": B.encode(&raw_id),
            "rawId": B.encode(&raw_id),
            "type": "public-key",
            "response": {
                "attestationObject": b64(&get(&resp, "attestationObject")),
                "clientDataJSON": b64(&get(&resp, "clientDataJSON")),
            },
            "extensions": {},
        }),
        raw_id,
        prf_enabled,
        prf_output,
    })
}

pub struct Asserted {
    pub credential: Value,
    pub raw_id: Vec<u8>,
    pub user_handle: Vec<u8>,
    pub prf_output: Option<Vec<u8>>,
}

/// `options` from the server, or None for an offline unlock: nobody checks the signature then,
/// and the proof is the PRF output itself, which unwraps the DEK.
pub async fn get_assertion(options: Option<&Value>, only: Option<&[u8]>) -> Result<Asserted, String> {
    let pk: JsValue = match options {
        Some(o) => {
            let root = to_js(o)?;
            let pk = get(&root, "publicKey");
            decode_field(&pk, "challenge")?;
            decode_ids(&pk, "allowCredentials")?;
            pk
        }
        None => {
            let mut c = [0u8; 32];
            getrandom::fill(&mut c).map_err(jerr)?;
            let pk: JsValue = Object::new().into();
            set(&pk, "challenge", &to_buf(&c))?;
            set(&pk, "userVerification", &"required".into())?;
            pk
        }
    };
    if let Some(id) = only {
        let d: JsValue = Object::new().into();
        set(&d, "type", &"public-key".into())?;
        set(&d, "id", &to_buf(id))?;
        set(&pk, "allowCredentials", &Array::of1(&d))?;
    }
    let ext = get(&pk, "extensions");
    let ext = if ext.is_object() { ext } else { Object::new().into() };
    set(&ext, "prf", &prf_ext(true)?)?;
    set(&pk, "extensions", &ext)?;

    let opts = web_sys::CredentialRequestOptions::from(JsValue::from(Object::new()));
    set(&opts, "publicKey", &pk)?;
    let cred = JsFuture::from(credentials()?.get_with_options(&opts).map_err(e)?).await.map_err(e)?;
    let resp = get(&cred, "response");
    let raw_id = bytes(&get(&cred, "rawId"));
    let user_handle = bytes(&get(&resp, "userHandle"));
    let (_, prf_output) = prf_output(&cred);
    Ok(Asserted {
        credential: json!({
            "id": B.encode(&raw_id),
            "rawId": B.encode(&raw_id),
            "type": "public-key",
            "response": {
                "authenticatorData": b64(&get(&resp, "authenticatorData")),
                "clientDataJSON": b64(&get(&resp, "clientDataJSON")),
                "signature": b64(&get(&resp, "signature")),
                "userHandle": B.encode(&user_handle),
            },
            "extensions": {},
        }),
        raw_id,
        user_handle,
        prf_output,
    })
}
