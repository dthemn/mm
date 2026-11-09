//! Client-server wire types. These fields are all the server ever sees.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use uuid::Uuid;

/// Bytes in JSON as base64url.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct B64(pub Vec<u8>);

impl Serialize for B64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for B64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        URL_SAFE_NO_PAD.decode(s).map(B64).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RegisterStartReq {
    /// Passkey label on the device. Never stored on the server.
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChallengeResp {
    pub challenge_id: Uuid,
    pub user_id: Option<Uuid>,
    pub options: Value,
    /// Random value to sign with the phrase key.
    pub nonce: Option<B64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RegisterFinishReq {
    pub challenge_id: Uuid,
    pub credential: Value,
    pub wrapped_dek_prf: Option<B64>,
    pub wrapped_dek_phrase: B64,
    pub phrase_pub: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoginFinishReq {
    pub challenge_id: Uuid,
    pub credential: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResp {
    pub user_id: Uuid,
    /// DEK copy under this passkey's PRF. Empty for a device without PRF.
    pub wrapped_dek_prf: Option<B64>,
    pub wrapped_dek_phrase: Option<B64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverStartReq {
    pub phrase_pub: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverStartResp {
    pub challenge_id: Uuid,
    pub nonce: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecoverFinishReq {
    pub challenge_id: Uuid,
    pub signature: B64,
}

/// Adds the DEK copy under a passkey's PRF to a passkey registered without one, the first time
/// the passkey returns PRF on a device that can unlock the key. Never replaces an existing copy.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyPrfReq {
    pub cred_id: B64,
    pub wrapped_dek_prf: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PasskeyAddFinishReq {
    pub challenge_id: Uuid,
    pub credential: Value,
    pub wrapped_dek_prf: Option<B64>,
    /// Phrase key signature: adding a passkey requires knowing the 12 words.
    pub signature: B64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub id: Uuid,
    pub revision: u64,
    pub seq: u64,
    pub blob: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullResp {
    pub changes: Vec<Change>,
    pub next: u64,
    pub more: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushItem {
    pub id: Uuid,
    pub expected_revision: u64,
    pub blob: B64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushReq {
    pub items: Vec<PushItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PushResult {
    Ok { id: Uuid, revision: u64 },
    Conflict { id: Uuid, current: Change },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PushResp {
    pub results: Vec<PushResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

/// One NBP average rate (table A). `rate_e4` is the rate in ten-thousandths: 4.3012 is 43012.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rate {
    pub date: String,
    pub rate_e4: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RatesResp {
    pub currency: String,
    pub rates: Vec<Rate>,
}
