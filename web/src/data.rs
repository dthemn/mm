//! Decrypted data in memory. It is dropped on lock.

use crate::flows::Session;
use mm_core::analytics::Tx;
use mm_core::model::*;
use mm_core::record::{Envelope, FieldConflict};
use mm_core::sync::{Keys, LocalStore, MANIFEST_ID, current, save};
use serde::Serialize;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    pub pending: bool,
    pub conflicts: Vec<FieldConflict>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    pub txs: Vec<Tx>,
    pub meta: HashMap<Uuid, Meta>,
    pub categories: HashMap<Uuid, Category>,
    pub accounts: HashMap<Uuid, Account>,
    pub merchants: HashMap<Uuid, Merchant>,
}

fn sorted<T: Clone>(m: &HashMap<Uuid, T>, name: impl Fn(&T) -> String) -> Vec<(Uuid, T)> {
    let mut v: Vec<(Uuid, T)> = m.iter().map(|(k, x)| (*k, x.clone())).collect();
    v.sort_by_key(|x| (name(&x.1).to_lowercase(), x.0));
    v
}

impl Data {
    /// Categories a user can pick: merged ones are hidden.
    pub fn active_categories(&self) -> Vec<(Uuid, Category)> {
        let mut v = sorted(&self.categories, |c| c.name.clone());
        v.retain(|c| c.1.merged_into.is_none());
        v
    }

    pub fn account_list(&self) -> Vec<(Uuid, Account)> {
        sorted(&self.accounts, |a| a.name.clone())
    }

    pub fn merchant_list(&self) -> Vec<(Uuid, Merchant)> {
        sorted(&self.merchants, |m| m.name.clone())
    }

    pub fn category_name(&self, id: Uuid) -> String {
        let c = mm_core::analytics::canonical(&self.categories, id);
        self.categories.get(&c).map(|c| c.name.clone()).unwrap_or_else(|| "Unknown".into())
    }

    pub fn merchant_name(&self, id: Option<Uuid>) -> Option<String> {
        id.and_then(|i| self.merchants.get(&i)).map(|m| m.name.clone())
    }

    pub fn conflicts(&self) -> usize {
        self.meta.values().filter(|m| !m.conflicts.is_empty()).count()
    }
}

/// Decrypts every record. Returns the data and the ids that failed to decrypt.
pub async fn load(s: &Session) -> Result<(Data, Vec<Uuid>), String> {
    let mut d = Data {
        categories: default_categories().into_iter().collect(),
        accounts: default_accounts().into_iter().collect(),
        ..Default::default()
    };
    let mut bad = vec![];
    let keys = Keys { user_id: s.user, dek: &s.dek };
    for r in s.store.all().await?.iter().filter(|r| r.id != MANIFEST_ID) {
        let Ok(env) = current(&keys, r) else {
            bad.push(r.id);
            continue;
        };
        let Some(e) = env else { continue };
        let parsed = apply(&mut d, r.id, &e);
        if !parsed {
            bad.push(r.id);
            continue;
        }
        if e.kind == KIND_TRANSACTION && !e.deleted {
            d.meta.insert(r.id, Meta { pending: r.pending_blob.is_some(), conflicts: e.conflicts.clone() });
        }
    }
    Ok((d, bad))
}

fn apply(d: &mut Data, id: Uuid, e: &Envelope) -> bool {
    let v = e.data.clone();
    match e.kind.as_str() {
        KIND_TRANSACTION => {
            if e.deleted {
                return true;
            }
            match serde_json::from_value(v) {
                Ok(t) => d.txs.push((id, t)),
                Err(_) => return false,
            }
        }
        KIND_CATEGORY => match serde_json::from_value(v) {
            Ok(c) => {
                d.categories.insert(id, c);
            }
            Err(_) => return false,
        },
        KIND_ACCOUNT => {
            if e.deleted {
                d.accounts.remove(&id);
                return true;
            }
            match serde_json::from_value(v) {
                Ok(a) => {
                    d.accounts.insert(id, a);
                }
                Err(_) => return false,
            }
        }
        KIND_MERCHANT => {
            if e.deleted {
                d.merchants.remove(&id);
                return true;
            }
            match serde_json::from_value(v) {
                Ok(m) => {
                    d.merchants.insert(id, m);
                }
                Err(_) => return false,
            }
        }
        _ => {}
    }
    true
}

pub async fn stored(s: &Session, id: Uuid) -> Option<Envelope> {
    let r = s.store.get(id).await.ok()??;
    current(&Keys { user_id: s.user, dek: &s.dek }, &r).ok()?
}

/// Writes new contents and keeps any open conflict on the record.
pub async fn write<T: Serialize>(s: &Session, id: Uuid, kind: &str, value: &T) -> Result<(), String> {
    let data = serde_json::to_value(value).map_err(|e| e.to_string())?;
    let env = match stored(s, id).await {
        Some(mut e) => {
            e.data = data;
            e.deleted = false;
            e
        }
        None => Envelope::new(kind, data),
    };
    save(&*s.store, &Keys { user_id: s.user, dek: &s.dek }, id, &env).await
}

pub async fn remove(s: &Session, id: Uuid, kind: &str, fallback: serde_json::Value) -> Result<(), String> {
    let mut e = stored(s, id).await.unwrap_or_else(|| Envelope::new(kind, fallback));
    e.deleted = true;
    save(&*s.store, &Keys { user_id: s.user, dek: &s.dek }, id, &e).await
}

pub async fn resolve(s: &Session, id: Uuid, field: &str, keep_theirs: bool) -> Result<(), String> {
    let mut e = stored(s, id).await.ok_or("record not found")?;
    e.resolve(field, keep_theirs);
    save(&*s.store, &Keys { user_id: s.user, dek: &s.dek }, id, &e).await
}
