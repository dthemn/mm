//! The local copy of the data. Only ciphertext reaches IndexedDB.

use js_sys::{Array, Function, Object, Promise, Reflect};
use mm_core::api::B64;
use mm_core::sync::{LocalRecord, LocalStore};
use rexie::{ObjectStore, Rexie, TransactionMode};
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, rc::Rc};
use uuid::Uuid;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;

const RECORDS: &str = "records";
const META: &str = "meta";
const CURSOR: &str = "cursor";

#[derive(Serialize, Deserialize)]
struct Stored {
    base_revision: u64,
    base_blob: Option<B64>,
    pending_blob: Option<B64>,
}

fn e<T: std::fmt::Debug>(x: T) -> String {
    format!("{x:?}")
}

/// The Web Lock is held while this value lives. It also works across tabs.
pub struct WebLock(Function);

impl Drop for WebLock {
    fn drop(&mut self) {
        let _ = self.0.call0(&JsValue::NULL);
    }
}

pub async fn web_lock(name: &str) -> Result<WebLock, String> {
    let win = web_sys::window().ok_or("no window")?;
    let locks = Reflect::get(&win.navigator(), &"locks".into()).map_err(e)?;
    let request: Function = Reflect::get(&locks, &"request".into()).map_err(e)?.dyn_into().map_err(e)?;

    let mut release = None;
    let hold = Promise::new(&mut |res, _| release = Some(res));
    let release = release.ok_or("no release function")?;
    let mut acquired = None;
    let granted = Promise::new(&mut |res, _| acquired = Some(res));
    let acquired = acquired.ok_or("no acquire function")?;

    let cb = Closure::once_into_js(move |_lock: JsValue| -> Promise {
        let _ = acquired.call0(&JsValue::NULL);
        hold
    });
    let _ = request.call2(&locks, &name.into(), &cb).map_err(e)?;
    JsFuture::from(granted).await.map_err(e)?;
    Ok(WebLock(release))
}

pub struct Store {
    db: RefCell<Option<Rc<Rexie>>>,
    lock: String,
}

fn db_name(user: Uuid) -> String {
    format!("mm-{user}")
}

impl Store {
    pub async fn open(user: Uuid) -> Result<Self, String> {
        let db = Rexie::builder(&db_name(user))
            .version(1)
            .add_object_store(ObjectStore::new(RECORDS))
            .add_object_store(ObjectStore::new(META))
            .build().await.map_err(e)?;
        Ok(Self { db: RefCell::new(Some(Rc::new(db))), lock: format!("mm-local-{user}") })
    }

    fn db(&self) -> Result<Rc<Rexie>, String> {
        self.db.borrow().clone().ok_or_else(|| "storage is closed".to_string())
    }

    /// deleteDatabase never completes while a connection is open, so the local
    /// copy can only be erased after this. Waits for sync and local writes first.
    pub async fn close(&self) -> Result<(), String> {
        let _sync = self.sync_lock().await?;
        let _local = web_lock(&self.lock).await?;
        if let Some(db) = self.db.borrow_mut().take() {
            Rc::try_unwrap(db).map_err(|_| "storage is still in use".to_string())?.close();
        }
        Ok(())
    }

    pub async fn destroy(user: Uuid) -> Result<(), String> {
        Rexie::delete(&db_name(user)).await.map_err(e)
    }

    /// Only one sync process per user.
    pub async fn sync_lock(&self) -> Result<WebLock, String> {
        web_lock(&format!("{}-sync", self.lock)).await
    }
}

fn to_record(id: Uuid, json: &str) -> Result<LocalRecord, String> {
    let s: Stored = serde_json::from_str(json).map_err(e)?;
    Ok(LocalRecord {
        id,
        base_revision: s.base_revision,
        base_blob: s.base_blob.map(|b| b.0),
        pending_blob: s.pending_blob.map(|b| b.0),
    })
}

impl LocalStore for Store {
    type Guard = WebLock;

    async fn lock(&self) -> Result<WebLock, String> {
        web_lock(&self.lock).await
    }

    async fn get(&self, id: Uuid) -> Result<Option<LocalRecord>, String> {
        let db = self.db()?;
        let tx = db.transaction(&[RECORDS], TransactionMode::ReadOnly).map_err(e)?;
        let v = tx.store(RECORDS).map_err(e)?.get(JsValue::from_str(&id.to_string())).await.map_err(e)?;
        v.and_then(|v| v.as_string()).map(|j| to_record(id, &j)).transpose()
    }

    async fn put(&self, r: LocalRecord) -> Result<(), String> {
        let s = Stored {
            base_revision: r.base_revision,
            base_blob: r.base_blob.map(B64),
            pending_blob: r.pending_blob.map(B64),
        };
        let db = self.db()?;
        let tx = db.transaction(&[RECORDS], TransactionMode::ReadWrite).map_err(e)?;
        tx.store(RECORDS).map_err(e)?
            .put(&JsValue::from_str(&serde_json::to_string(&s).map_err(e)?), Some(&JsValue::from_str(&r.id.to_string())))
            .await.map_err(e)?;
        tx.done().await.map_err(e)?;
        Ok(())
    }

    async fn all(&self) -> Result<Vec<LocalRecord>, String> {
        let db = self.db()?;
        let tx = db.transaction(&[RECORDS], TransactionMode::ReadOnly).map_err(e)?;
        let store = tx.store(RECORDS).map_err(e)?;
        let keys = store.get_all_keys(None, None).await.map_err(e)?;
        let vals = store.get_all(None, None).await.map_err(e)?;
        keys.iter().zip(vals.iter())
            .map(|(k, v)| {
                let id: Uuid = k.as_string().unwrap_or_default().parse().map_err(e)?;
                to_record(id, &v.as_string().unwrap_or_default())
            })
            .collect()
    }

    async fn cursor(&self) -> Result<u64, String> {
        let db = self.db()?;
        let tx = db.transaction(&[META], TransactionMode::ReadOnly).map_err(e)?;
        let v = tx.store(META).map_err(e)?.get(JsValue::from_str(CURSOR)).await.map_err(e)?;
        Ok(v.and_then(|v| v.as_f64()).unwrap_or(0.0) as u64)
    }

    async fn set_cursor(&self, seq: u64) -> Result<(), String> {
        let db = self.db()?;
        let tx = db.transaction(&[META], TransactionMode::ReadWrite).map_err(e)?;
        tx.store(META).map_err(e)?.put(&JsValue::from_f64(seq as f64), Some(&JsValue::from_str(CURSOR))).await.map_err(e)?;
        tx.done().await.map_err(e)?;
        Ok(())
    }
}

/// Key-value store for device data (`mm-device`). Values hold no plaintext.
pub struct Kv(Rexie);

impl Kv {
    pub async fn open() -> Result<Self, String> {
        Rexie::builder("mm-device").version(1).add_object_store(ObjectStore::new("kv")).build().await.map(Self).map_err(e)
    }
    pub async fn get(&self, k: &str) -> Result<Option<JsValue>, String> {
        let tx = self.0.transaction(&["kv"], TransactionMode::ReadOnly).map_err(e)?;
        tx.store("kv").map_err(e)?.get(JsValue::from_str(k)).await.map_err(e)
    }
    pub async fn put(&self, k: &str, v: &JsValue) -> Result<(), String> {
        let tx = self.0.transaction(&["kv"], TransactionMode::ReadWrite).map_err(e)?;
        tx.store("kv").map_err(e)?.put(v, Some(&JsValue::from_str(k))).await.map_err(e)?;
        tx.done().await.map_err(e)?;
        Ok(())
    }
    pub async fn del(&self, k: &str) -> Result<(), String> {
        let tx = self.0.transaction(&["kv"], TransactionMode::ReadWrite).map_err(e)?;
        tx.store("kv").map_err(e)?.delete(JsValue::from_str(k)).await.map_err(e)?;
        tx.done().await.map_err(e)?;
        Ok(())
    }
}

pub fn js_obj(pairs: &[(&str, JsValue)]) -> Object {
    let o = Object::new();
    for (k, v) in pairs {
        let _ = Reflect::set(&o, &JsValue::from_str(k), v);
    }
    o
}

pub fn js_arr(items: &[&str]) -> Array {
    items.iter().map(|s| JsValue::from_str(s)).collect()
}
