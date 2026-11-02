//! Sync. All local state is encrypted, so the queue needs no key to be stored,
//! and losing the connection at any moment loses no data:
//! a local edit is removed only after the server confirms it.

use crate::api::{B64, Change, PullResp, PushItem, PushResult};
use crate::crypto::{CryptoError, Dek, RecordAad, open_record, seal_record};
use crate::record::{Envelope, KIND_MANIFEST, SCHEMA, merge3};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;

const PUSH_BATCH: usize = 50;
/// Limit on one request body, so a large batch cannot get stuck forever.
const PUSH_MAX_BYTES: usize = 8 << 20;
const MAX_PUSH_ROUNDS: usize = 8;

/// The manifest has a fixed id: all devices write to the same record.
pub const MANIFEST_ID: Uuid = Uuid::from_u128(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalRecord {
    pub id: Uuid,
    /// The last revision confirmed by the server. 0 means it has never been there.
    pub base_revision: u64,
    pub base_blob: Option<Vec<u8>>,
    /// An unsent edit, encrypted for revision `base_revision + 1`.
    pub pending_blob: Option<Vec<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError<E> {
    #[error("network: {0}")]
    Remote(E),
    #[error("local storage: {0}")]
    Local(String),
    #[error(transparent)]
    Crypto(#[from] CryptoError),
    #[error("invalid record data")]
    Format,
    #[error("sync did not converge in {MAX_PUSH_ROUNDS} attempts")]
    NotConverged,
}

#[allow(async_fn_in_trait)]
pub trait LocalStore {
    /// Held while reading and writing one record, so that a user edit
    /// cannot get lost between get and put. Never held across network calls.
    type Guard;
    async fn lock(&self) -> Result<Self::Guard, String>;
    async fn get(&self, id: Uuid) -> Result<Option<LocalRecord>, String>;
    async fn put(&self, rec: LocalRecord) -> Result<(), String>;
    async fn all(&self) -> Result<Vec<LocalRecord>, String>;
    async fn cursor(&self) -> Result<u64, String>;
    async fn set_cursor(&self, seq: u64) -> Result<(), String>;
}

#[allow(async_fn_in_trait)]
pub trait Remote {
    type Error;
    async fn pull(&self, since: u64) -> Result<PullResp, Self::Error>;
    async fn push(&self, items: Vec<PushItem>) -> Result<Vec<PushResult>, Self::Error>;
}

/// A sign that the server tampered with something. Such data is not applied,
/// the local copy stays, and the user sees a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntegrityIssue {
    BadCiphertext(Uuid),
    Rollback { id: Uuid, known: u64, got: u64 },
    Forked { id: Uuid, revision: u64 },
    Missing { id: Uuid, expected: u64, have: u64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub pulled: usize,
    pub pushed: usize,
    pub merged: usize,
    pub issues: Vec<IntegrityIssue>,
}

pub struct Keys<'a> {
    pub user_id: Uuid,
    pub dek: &'a Dek,
}

impl Keys<'_> {
    fn aad(&self, id: Uuid, revision: u64) -> RecordAad {
        RecordAad { user_id: self.user_id, record_id: id, schema: SCHEMA, revision }
    }

    pub fn seal(&self, id: Uuid, revision: u64, env: &Envelope) -> Result<Vec<u8>, CryptoError> {
        let plain = zeroize::Zeroizing::new(serde_json::to_vec(env).map_err(|_| CryptoError::Format)?);
        seal_record(self.dek, self.aad(id, revision), &plain)
    }

    pub fn open(&self, id: Uuid, revision: u64, blob: &[u8]) -> Result<Envelope, CryptoError> {
        let plain = open_record(self.dek, self.aad(id, revision), blob)?;
        serde_json::from_slice(&plain).map_err(|_| CryptoError::Format)
    }
}

/// Current record contents: the unsent edit, otherwise the server version.
pub fn current(keys: &Keys, r: &LocalRecord) -> Result<Option<Envelope>, CryptoError> {
    if let Some(p) = &r.pending_blob {
        return keys.open(r.id, r.base_revision + 1, p).map(Some);
    }
    r.base_blob.as_ref().map(|b| keys.open(r.id, r.base_revision, b)).transpose()
}

/// Saves a local edit. No network needed.
pub async fn save<L: LocalStore>(local: &L, keys: &Keys<'_>, id: Uuid, env: &Envelope) -> Result<(), String> {
    let _guard = local.lock().await?;
    let mut r = local.get(id).await?.unwrap_or(LocalRecord {
        id,
        base_revision: 0,
        base_blob: None,
        pending_blob: None,
    });
    r.pending_blob = Some(keys.seal(id, r.base_revision + 1, env).map_err(|e| e.to_string())?);
    local.put(r).await
}

fn merge_manifest(a: &Envelope, b: &Envelope) -> Envelope {
    let mut revs = manifest_revs(a);
    for (k, v) in manifest_revs(b) {
        let e = revs.entry(k).or_insert(0);
        *e = (*e).max(v);
    }
    manifest_env(&revs)
}

fn manifest_revs(e: &Envelope) -> BTreeMap<Uuid, u64> {
    e.data
        .get("revs")
        .and_then(Value::as_object)
        .map(|m| m.iter().filter_map(|(k, v)| Some((k.parse().ok()?, v.as_u64()?))).collect())
        .unwrap_or_default()
}

fn manifest_env(revs: &BTreeMap<Uuid, u64>) -> Envelope {
    let map: Map<String, Value> = revs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect();
    Envelope::new(KIND_MANIFEST, json!({ "revs": map }))
}

pub struct Syncer<'a, L, R> {
    pub local: &'a L,
    pub remote: &'a R,
    pub keys: Keys<'a>,
}

impl<L: LocalStore, R: Remote> Syncer<'_, L, R> {
    fn local_err(e: String) -> SyncError<R::Error> {
        SyncError::Local(e)
    }

    /// Applies a server version. An unsent edit is merged with it.
    async fn apply(&self, ch: &Change, rep: &mut SyncReport) -> Result<(), SyncError<R::Error>> {
        let keys = &self.keys;
        let _guard = self.local.lock().await.map_err(Self::local_err)?;
        let mut r = self.local.get(ch.id).await.map_err(Self::local_err)?.unwrap_or(LocalRecord {
            id: ch.id,
            base_revision: 0,
            base_blob: None,
            pending_blob: None,
        });
        if ch.revision < r.base_revision {
            rep.issues.push(IntegrityIssue::Rollback { id: ch.id, known: r.base_revision, got: ch.revision });
            return Ok(());
        }
        if ch.revision == r.base_revision {
            if r.base_blob.as_deref() != Some(&ch.blob.0[..]) {
                rep.issues.push(IntegrityIssue::Forked { id: ch.id, revision: ch.revision });
            }
            return Ok(());
        }
        // Our edit was accepted, but the confirmation got lost.
        if ch.revision == r.base_revision + 1 && r.pending_blob.as_deref() == Some(&ch.blob.0[..]) {
            r.base_revision = ch.revision;
            r.base_blob = r.pending_blob.take();
            self.local.put(r).await.map_err(Self::local_err)?;
            return Ok(());
        }
        let theirs = match keys.open(ch.id, ch.revision, &ch.blob.0) {
            Ok(e) => e,
            Err(_) => {
                rep.issues.push(IntegrityIssue::BadCiphertext(ch.id));
                return Ok(());
            }
        };
        if let Some(pending) = r.pending_blob.take() {
            let mine = keys.open(ch.id, r.base_revision + 1, &pending)?;
            let merged = if mine.kind == KIND_MANIFEST {
                merge_manifest(&mine, &theirs)
            } else {
                let base = r.base_blob.as_ref().map(|b| keys.open(ch.id, r.base_revision, b)).transpose()?;
                merge3(base.as_ref(), &mine, &theirs)
            };
            r.pending_blob = Some(keys.seal(ch.id, ch.revision + 1, &merged)?);
            rep.merged += 1;
        }
        r.base_revision = ch.revision;
        r.base_blob = Some(ch.blob.0.clone());
        self.local.put(r).await.map_err(Self::local_err)?;
        rep.pulled += 1;
        Ok(())
    }

    async fn pull_all(&self, rep: &mut SyncReport) -> Result<(), SyncError<R::Error>> {
        let mut since = self.local.cursor().await.map_err(Self::local_err)?;
        loop {
            let page = self.remote.pull(since).await.map_err(SyncError::Remote)?;
            for ch in &page.changes {
                self.apply(ch, rep).await?;
            }
            // The cursor moves only after applying: after a failure the page simply comes again.
            self.local.set_cursor(page.next).await.map_err(Self::local_err)?;
            since = page.next;
            if !page.more {
                return Ok(());
            }
        }
    }

    /// The server accepted a write. If the user edited the record again during the push,
    /// the newer edit is kept and re-encrypted for the next revision.
    async fn ack(&self, id: Uuid, revision: u64, sent: Option<&Vec<u8>>, rep: &mut SyncReport) -> Result<(), SyncError<R::Error>> {
        let _guard = self.local.lock().await.map_err(Self::local_err)?;
        let Some(mut r) = self.local.get(id).await.map_err(Self::local_err)? else { return Ok(()) };
        let Some(sent) = sent else { return Ok(()) };
        let newer = match r.pending_blob.take() {
            Some(p) if p != *sent => Some(self.keys.open(id, r.base_revision + 1, &p)?),
            _ => None,
        };
        r.base_revision = revision;
        r.base_blob = Some(sent.clone());
        if let Some(env) = newer {
            r.pending_blob = Some(self.keys.seal(id, revision + 1, &env)?);
        }
        self.local.put(r).await.map_err(Self::local_err)?;
        rep.pushed += 1;
        Ok(())
    }

    async fn push_pending(&self, rep: &mut SyncReport) -> Result<(), SyncError<R::Error>> {
        for _ in 0..MAX_PUSH_ROUNDS {
            let all = self.local.all().await.map_err(Self::local_err)?;
            let pending: Vec<&LocalRecord> = all.iter().filter(|r| r.pending_blob.is_some()).collect();
            if pending.is_empty() {
                return Ok(());
            }
            let mut batches: Vec<Vec<PushItem>> = vec![];
            let mut bytes = 0;
            for r in pending {
                let blob = r.pending_blob.clone().unwrap_or_default();
                let full = batches.last().is_none_or(|b| b.len() >= PUSH_BATCH || bytes + blob.len() > PUSH_MAX_BYTES);
                if full {
                    batches.push(vec![]);
                    bytes = 0;
                }
                bytes += blob.len();
                batches.last_mut().expect("a batch was just added").push(PushItem {
                    id: r.id,
                    expected_revision: r.base_revision,
                    blob: B64(blob),
                });
            }
            for items in batches {
                let sent: BTreeMap<Uuid, Vec<u8>> = items.iter().map(|i| (i.id, i.blob.0.clone())).collect();
                for res in self.remote.push(items).await.map_err(SyncError::Remote)? {
                    match res {
                        PushResult::Ok { id, revision } => {
                            self.ack(id, revision, sent.get(&id), rep).await?;
                        }
                        PushResult::Conflict { current, .. } => self.apply(&current, rep).await?,
                    }
                }
            }
        }
        Err(SyncError::NotConverged)
    }

    /// Writes the revisions of all records to the manifest if they changed.
    async fn update_manifest(&self) -> Result<(), SyncError<R::Error>> {
        let all = self.local.all().await.map_err(Self::local_err)?;
        let revs: BTreeMap<Uuid, u64> = all
            .iter()
            .filter(|r| r.id != MANIFEST_ID && r.base_revision > 0)
            .map(|r| (r.id, r.base_revision))
            .collect();
        let old = all.iter().find(|r| r.id == MANIFEST_ID);
        let cur = old.map(|r| current(&self.keys, r)).transpose()?.flatten();
        let merged = match &cur {
            Some(c) => merge_manifest(c, &manifest_env(&revs)),
            None => manifest_env(&revs),
        };
        if cur.as_ref().map(|c| c.data == merged.data).unwrap_or(revs.is_empty()) {
            return Ok(());
        }
        save(self.local, &self.keys, MANIFEST_ID, &merged).await.map_err(Self::local_err)
    }

    /// The server cannot hide a record or return an old version unnoticed.
    async fn verify_manifest(&self, rep: &mut SyncReport) -> Result<(), SyncError<R::Error>> {
        let all = self.local.all().await.map_err(Self::local_err)?;
        let Some(m) = all.iter().find(|r| r.id == MANIFEST_ID) else { return Ok(()) };
        let Some(env) = current(&self.keys, m)? else { return Ok(()) };
        for (id, expected) in manifest_revs(&env) {
            let have = all.iter().find(|r| r.id == id).map(|r| r.base_revision).unwrap_or(0);
            if have < expected {
                rep.issues.push(IntegrityIssue::Missing { id, expected, have });
            }
        }
        Ok(())
    }

    pub async fn run(&self) -> Result<SyncReport, SyncError<R::Error>> {
        let mut rep = SyncReport::default();
        self.pull_all(&mut rep).await?;
        self.verify_manifest(&mut rep).await?;
        self.push_pending(&mut rep).await?;
        self.update_manifest().await?;
        self.push_pending(&mut rep).await?;
        Ok(rep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::FIELD_DELETED;
    use pollster::block_on;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Mem {
        recs: RefCell<BTreeMap<Uuid, LocalRecord>>,
        cursor: Cell<u64>,
    }

    impl LocalStore for Mem {
        type Guard = ();
        async fn lock(&self) -> Result<(), String> {
            Ok(())
        }
        async fn get(&self, id: Uuid) -> Result<Option<LocalRecord>, String> {
            Ok(self.recs.borrow().get(&id).cloned())
        }
        async fn put(&self, rec: LocalRecord) -> Result<(), String> {
            self.recs.borrow_mut().insert(rec.id, rec);
            Ok(())
        }
        async fn all(&self) -> Result<Vec<LocalRecord>, String> {
            Ok(self.recs.borrow().values().cloned().collect())
        }
        async fn cursor(&self) -> Result<u64, String> {
            Ok(self.cursor.get())
        }
        async fn set_cursor(&self, seq: u64) -> Result<(), String> {
            self.cursor.set(seq);
            Ok(())
        }
    }

    /// A server with the same compare-and-swap semantics as the real one.
    #[derive(Default)]
    struct Server {
        recs: RefCell<BTreeMap<Uuid, Change>>,
        seq: Cell<u64>,
        /// Accept the write but "lose" the response.
        lose_ack: Cell<bool>,
        offline: Cell<bool>,
        pushes: RefCell<Vec<usize>>,
        /// A user edit made exactly during the push.
        during_push: RefCell<Option<Box<dyn Fn()>>>,
    }

    impl Remote for Server {
        type Error = String;
        async fn pull(&self, since: u64) -> Result<PullResp, String> {
            if self.offline.get() {
                return Err("offline".into());
            }
            let mut changes: Vec<Change> =
                self.recs.borrow().values().filter(|c| c.seq > since).cloned().collect();
            changes.sort_by_key(|c| c.seq);
            changes.truncate(2); // paged, to exercise the cursor
            let more = self.recs.borrow().values().filter(|c| c.seq > since).count() > changes.len();
            Ok(PullResp { next: changes.last().map(|c| c.seq).unwrap_or(since), changes, more })
        }
        async fn push(&self, items: Vec<PushItem>) -> Result<Vec<PushResult>, String> {
            if self.offline.get() {
                return Err("offline".into());
            }
            self.pushes.borrow_mut().push(items.iter().map(|i| i.blob.0.len()).sum());
            if let Some(f) = self.during_push.borrow_mut().take() {
                f();
            }
            let mut out = vec![];
            for it in items {
                let cur = self.recs.borrow().get(&it.id).cloned();
                let rev = cur.as_ref().map(|c| c.revision).unwrap_or(0);
                if let Some(c) = &cur
                    && c.revision == it.expected_revision + 1
                    && c.blob == it.blob
                {
                    out.push(PushResult::Ok { id: it.id, revision: c.revision });
                    continue;
                }
                if rev != it.expected_revision {
                    out.push(PushResult::Conflict { id: it.id, current: cur.unwrap() });
                    continue;
                }
                self.seq.set(self.seq.get() + 1);
                let revision = rev + 1;
                self.recs.borrow_mut().insert(
                    it.id,
                    Change { id: it.id, revision, seq: self.seq.get(), blob: it.blob },
                );
                out.push(PushResult::Ok { id: it.id, revision });
            }
            if self.lose_ack.replace(false) {
                return Err("connection dropped".into());
            }
            Ok(out)
        }
    }

    struct World {
        user: Uuid,
        dek: Dek,
        server: Server,
    }

    impl World {
        fn new() -> Self {
            Self { user: Uuid::new_v4(), dek: Dek::generate().unwrap(), server: Server::default() }
        }
        fn keys(&self) -> Keys<'_> {
            Keys { user_id: self.user, dek: &self.dek }
        }
        fn sync(&self, d: &Mem) -> Result<SyncReport, SyncError<String>> {
            block_on(Syncer { local: d, remote: &self.server, keys: self.keys() }.run())
        }
        fn write(&self, d: &Mem, id: Uuid, v: Value) {
            block_on(save(d, &self.keys(), id, &Envelope::new("note", v))).unwrap();
        }
        fn read(&self, d: &Mem, id: Uuid) -> Envelope {
            let r = d.recs.borrow().get(&id).cloned().unwrap();
            current(&self.keys(), &r).unwrap().unwrap()
        }
    }

    #[test]
    fn record_reaches_second_device() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"t": "x"}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        assert_eq!(w.read(&b, id).data, json!({"t": "x"}));
    }

    #[test]
    fn paging_delivers_everything() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let ids: Vec<Uuid> = (0..7).map(|_| Uuid::new_v4()).collect();
        for id in &ids {
            w.write(&a, *id, json!({"t": 1}));
        }
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        for id in &ids {
            assert_eq!(w.read(&b, *id).data, json!({"t": 1}));
        }
    }

    #[test]
    fn offline_edit_survives_and_syncs_later() {
        let w = World::new();
        let a = Mem::default();
        let id = Uuid::new_v4();
        w.server.offline.set(true);
        w.write(&a, id, json!({"t": 1}));
        assert!(w.sync(&a).is_err());
        assert_eq!(w.read(&a, id).data, json!({"t": 1}));
        assert!(a.recs.borrow()[&id].pending_blob.is_some());
        w.server.offline.set(false);
        w.sync(&a).unwrap();
        assert!(a.recs.borrow()[&id].pending_blob.is_none());
    }

    #[test]
    fn lost_ack_does_not_create_a_conflict_or_lose_data() {
        let w = World::new();
        let a = Mem::default();
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"t": 1}));
        w.server.lose_ack.set(true);
        assert!(w.sync(&a).is_err());
        assert!(a.recs.borrow()[&id].pending_blob.is_some(), "the edit stays until confirmed");
        let rep = w.sync(&a).unwrap();
        assert_eq!(rep.merged, 0);
        assert!(w.read(&a, id).conflicts.is_empty());
        assert_eq!(a.recs.borrow()[&id].base_revision, 1);
    }

    #[test]
    fn edit_made_during_push_is_not_lost() {
        let w = World::new();
        let a = std::rc::Rc::new(Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"t": "first"}));
        let (user, dek, d) = (w.user, w.dek.clone(), a.clone());
        *w.server.during_push.borrow_mut() = Some(Box::new(move || {
            let keys = Keys { user_id: user, dek: &dek };
            block_on(save(&*d, &keys, id, &Envelope::new("note", json!({"t": "second"})))).unwrap();
        }));
        w.sync(&a).unwrap();
        assert_eq!(w.read(&a, id).data, json!({"t": "second"}));
        w.sync(&a).unwrap();
        let b = Mem::default();
        w.sync(&b).unwrap();
        assert_eq!(w.read(&b, id).data, json!({"t": "second"}));
        assert!(a.recs.borrow()[&id].pending_blob.is_none());
    }

    #[test]
    fn large_records_are_split_into_bounded_requests() {
        let w = World::new();
        let a = Mem::default();
        for _ in 0..12 {
            w.write(&a, Uuid::new_v4(), json!({"t": "x".repeat(1 << 19)}));
        }
        w.sync(&a).unwrap();
        let pushes = w.server.pushes.borrow();
        assert!(pushes.len() > 1);
        assert!(pushes.iter().all(|n| *n <= PUSH_MAX_BYTES));
        assert!(a.recs.borrow().values().all(|r| r.pending_blob.is_none()));
    }

    #[test]
    fn concurrent_edits_of_different_fields_merge() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 1, "y": 1}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        w.write(&a, id, json!({"x": 2, "y": 1}));
        w.write(&b, id, json!({"x": 1, "y": 2}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        w.sync(&a).unwrap();
        for d in [&a, &b] {
            assert_eq!(w.read(d, id).data, json!({"x": 2, "y": 2}));
            assert!(w.read(d, id).conflicts.is_empty());
        }
    }

    #[test]
    fn concurrent_edits_of_one_field_keep_both_values_on_both_devices() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 0}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        w.write(&a, id, json!({"x": 1}));
        w.write(&b, id, json!({"x": 2}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        w.sync(&a).unwrap();
        for d in [&a, &b] {
            let e = w.read(d, id);
            assert_eq!(e.conflicts.len(), 1);
            let vals = [&e.conflicts[0].mine, &e.conflicts[0].theirs];
            assert!(vals.contains(&&json!(1)) && vals.contains(&&json!(2)));
        }
    }

    #[test]
    fn delete_against_offline_edit_loses_nothing() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 0}));
        w.sync(&a).unwrap();
        w.sync(&b).unwrap();
        let mut del = w.read(&a, id);
        del.deleted = true;
        block_on(save(&a, &w.keys(), id, &del)).unwrap();
        w.sync(&a).unwrap();
        w.write(&b, id, json!({"x": 5}));
        w.sync(&b).unwrap();
        let e = w.read(&b, id);
        assert!(!e.deleted);
        assert_eq!(e.data, json!({"x": 5}));
        assert_eq!(e.conflicts[0].field, FIELD_DELETED);
    }

    #[test]
    fn server_rollback_is_detected_and_not_applied() {
        let w = World::new();
        let a = Mem::default();
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 1}));
        w.sync(&a).unwrap();
        let old = w.server.recs.borrow()[&id].clone();
        w.write(&a, id, json!({"x": 2}));
        w.sync(&a).unwrap();
        // The server returns an old valid version under a new sequence number.
        w.server.seq.set(w.server.seq.get() + 1);
        w.server.recs.borrow_mut().insert(id, Change { seq: w.server.seq.get(), ..old });
        let rep = w.sync(&a).unwrap();
        assert!(rep.issues.iter().any(|i| matches!(i, IntegrityIssue::Rollback { .. })));
        assert_eq!(w.read(&a, id).data, json!({"x": 2}));
    }

    #[test]
    fn old_blob_under_a_new_revision_number_is_rejected() {
        let w = World::new();
        let a = Mem::default();
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 1}));
        w.sync(&a).unwrap();
        let old = w.server.recs.borrow()[&id].clone();
        w.server.seq.set(w.server.seq.get() + 1);
        let forged = Change { revision: 2, seq: w.server.seq.get(), ..old };
        w.server.recs.borrow_mut().insert(id, forged);
        let rep = w.sync(&a).unwrap();
        assert!(rep.issues.contains(&IntegrityIssue::BadCiphertext(id)));
        assert_eq!(w.read(&a, id).data, json!({"x": 1}));
    }

    #[test]
    fn swapped_records_are_rejected() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let (i, j) = (Uuid::new_v4(), Uuid::new_v4());
        w.write(&a, i, json!({"v": "i"}));
        w.write(&a, j, json!({"v": "j"}));
        w.sync(&a).unwrap();
        {
            let mut r = w.server.recs.borrow_mut();
            let (bi, bj) = (r[&i].blob.clone(), r[&j].blob.clone());
            r.get_mut(&i).unwrap().blob = bj;
            r.get_mut(&j).unwrap().blob = bi;
        }
        let rep = w.sync(&b).unwrap();
        assert!(rep.issues.contains(&IntegrityIssue::BadCiphertext(i)));
        assert!(rep.issues.contains(&IntegrityIssue::BadCiphertext(j)));
    }

    #[test]
    fn hidden_record_is_reported_by_manifest() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        let id = Uuid::new_v4();
        w.write(&a, id, json!({"x": 1}));
        w.sync(&a).unwrap();
        w.server.recs.borrow_mut().remove(&id);
        let rep = w.sync(&b).unwrap();
        assert!(rep.issues.iter().any(|i| matches!(i, IntegrityIssue::Missing { .. })));
    }

    #[test]
    fn devices_converge_on_the_manifest_without_extra_issues() {
        let w = World::new();
        let (a, b) = (Mem::default(), Mem::default());
        for _ in 0..3 {
            w.write(&a, Uuid::new_v4(), json!({"t": 1}));
            w.write(&b, Uuid::new_v4(), json!({"t": 2}));
            assert_eq!(w.sync(&a).unwrap().issues, vec![]);
            assert_eq!(w.sync(&b).unwrap().issues, vec![]);
        }
        assert_eq!(w.sync(&a).unwrap().issues, vec![]);
        assert!(a.recs.borrow().values().all(|r| r.pending_blob.is_none()));
    }

    #[test]
    fn repeated_sync_sends_nothing_new() {
        let w = World::new();
        let a = Mem::default();
        w.write(&a, Uuid::new_v4(), json!({"t": 1}));
        w.sync(&a).unwrap();
        let seq = w.server.seq.get();
        w.sync(&a).unwrap();
        assert_eq!(w.server.seq.get(), seq);
    }
}
