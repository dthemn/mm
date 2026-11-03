use mm_core::api::*;
use mm_core::sync::Remote;
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestCredentials, RequestInit, Response};

#[derive(Clone, Debug, PartialEq)]
pub enum ApiFail {
    /// The server is unreachable: work can continue offline.
    Network,
    Status(u16, String),
}

impl std::fmt::Display for ApiFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network => f.write_str("cannot reach the server"),
            Self::Status(401, _) => f.write_str("login required"),
            Self::Status(429, _) => f.write_str("too many attempts, wait a minute"),
            Self::Status(c, m) => write!(f, "server error {c}: {m}"),
        }
    }
}

async fn call(method: &str, path: &str, body: Option<String>) -> Result<Response, ApiFail> {
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::SameOrigin);
    if let Some(b) = body {
        init.set_body(&JsValue::from_str(&b));
    }
    let req = Request::new_with_str_and_init(path, &init).map_err(|_| ApiFail::Network)?;
    req.headers().set("Content-Type", "application/json").map_err(|_| ApiFail::Network)?;
    let win = web_sys::window().ok_or(ApiFail::Network)?;
    let resp: Response = JsFuture::from(win.fetch_with_request(&req)).await
        .map_err(|_| ApiFail::Network)?.dyn_into().map_err(|_| ApiFail::Network)?;
    if resp.ok() {
        return Ok(resp);
    }
    let text = match resp.text() {
        Ok(p) => JsFuture::from(p).await.ok().and_then(|v| v.as_string()).unwrap_or_default(),
        Err(_) => String::new(),
    };
    let msg = serde_json::from_str::<ApiError>(&text).map(|e| e.error).unwrap_or_default();
    Err(ApiFail::Status(resp.status(), msg))
}

async fn parse<T: DeserializeOwned>(r: Response) -> Result<T, ApiFail> {
    let p = r.text().map_err(|_| ApiFail::Network)?;
    let v = JsFuture::from(p).await.map_err(|_| ApiFail::Network)?;
    serde_json::from_str(&v.as_string().unwrap_or_default()).map_err(|_| ApiFail::Status(0, "response".into()))
}

pub async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiFail> {
    parse(call("GET", path, None).await?).await
}

pub async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, ApiFail> {
    let s = serde_json::to_string(body).map_err(|_| ApiFail::Network)?;
    parse(call("POST", path, Some(s)).await?).await
}

pub async fn post_only<B: Serialize>(path: &str, body: &B) -> Result<(), ApiFail> {
    let body = serde_json::to_string(body).map_err(|_| ApiFail::Status(0, "request".into()))?;
    call("POST", path, Some(body)).await.map(|_| ())
}

pub async fn post_empty<T: DeserializeOwned>(path: &str) -> Result<T, ApiFail> {
    parse(call("POST", path, Some("{}".into())).await?).await
}

pub async fn post_done(path: &str) -> Result<(), ApiFail> {
    call("POST", path, Some("{}".into())).await.map(|_| ())
}

pub struct Http;

impl Remote for Http {
    type Error = ApiFail;
    async fn pull(&self, since: u64) -> Result<PullResp, ApiFail> {
        parse(call("GET", &format!("/api/sync/pull?since={since}"), None).await?).await
    }
    async fn push(&self, items: Vec<PushItem>) -> Result<Vec<PushResult>, ApiFail> {
        let r: PushResp = post("/api/sync/push", &PushReq { items }).await?;
        Ok(r.results)
    }
}
