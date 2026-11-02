//! Record contents and three-way merge. The server never sees this data.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const SCHEMA: u32 = 1;
/// Internal record kind holding the list of revisions.
pub const KIND_MANIFEST: &str = "manifest";

/// A field changed on two devices. Both versions are kept until the user chooses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldConflict {
    pub field: String,
    pub mine: Value,
    pub theirs: Value,
}

/// Pseudo-field for a "deleted here, edited there" conflict.
pub const FIELD_DELETED: &str = "_deleted";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub kind: String,
    pub data: Value,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<FieldConflict>,
}

impl Envelope {
    pub fn new(kind: &str, data: Value) -> Self {
        Self { kind: kind.into(), data, deleted: false, conflicts: vec![] }
    }

    /// Applies the user's choice. `keep_theirs` takes the other device's version.
    pub fn resolve(&mut self, field: &str, keep_theirs: bool) {
        let Some(pos) = self.conflicts.iter().position(|c| c.field == field) else { return };
        let c = self.conflicts.remove(pos);
        if field == FIELD_DELETED {
            self.deleted = !keep_theirs;
            return;
        }
        let v = if keep_theirs { c.theirs } else { c.mine };
        match &mut self.data {
            Value::Object(m) if !field.is_empty() => {
                if v.is_null() {
                    m.remove(field);
                } else {
                    m.insert(field.to_string(), v);
                }
            }
            other => *other = v,
        }
    }
}

fn get(m: &Map<String, Value>, k: &str) -> Value {
    m.get(k).cloned().unwrap_or(Value::Null)
}

fn merge_conflicts(into: &mut Vec<FieldConflict>, from: &[FieldConflict]) {
    for c in from {
        if !into.contains(c) {
            into.push(c.clone());
        }
    }
}

/// Field-level merge. No change is ever lost: on a collision the value
/// `mine` stays in the data and both versions go to `conflicts`.
pub fn merge3(base: Option<&Envelope>, mine: &Envelope, theirs: &Envelope) -> Envelope {
    let empty = Envelope::new(&mine.kind, Value::Object(Map::new()));
    let base = base.unwrap_or(&empty);
    let mut conflicts = mine.conflicts.clone();
    merge_conflicts(&mut conflicts, &theirs.conflicts);

    // Delete against edit: keep the edit and show it to the user.
    if mine.deleted != theirs.deleted {
        let (edited, was_edited) = if mine.deleted {
            (theirs, theirs.data != base.data)
        } else {
            (mine, mine.data != base.data)
        };
        if !was_edited {
            return Envelope { deleted: true, conflicts, ..edited.clone() };
        }
        conflicts.push(FieldConflict {
            field: FIELD_DELETED.into(),
            mine: Value::Bool(mine.deleted),
            theirs: Value::Bool(theirs.deleted),
        });
        return Envelope { deleted: false, conflicts, ..edited.clone() };
    }

    let data = match (&base.data, &mine.data, &theirs.data) {
        (Value::Object(b), Value::Object(m), Value::Object(t)) => {
            let mut keys: Vec<&String> = m.keys().chain(t.keys()).chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            let mut out = Map::new();
            for k in keys {
                let (bv, mv, tv) = (get(b, k), get(m, k), get(t, k));
                let v = if mv == tv || tv == bv {
                    mv
                } else if mv == bv {
                    tv
                } else {
                    conflicts.push(FieldConflict { field: k.clone(), mine: mv.clone(), theirs: tv });
                    mv
                };
                if !v.is_null() {
                    out.insert(k.clone(), v);
                }
            }
            Value::Object(out)
        }
        (b, m, t) => {
            if m == t || t == b {
                m.clone()
            } else if m == b {
                t.clone()
            } else {
                conflicts.push(FieldConflict { field: String::new(), mine: m.clone(), theirs: t.clone() });
                m.clone()
            }
        }
    };
    Envelope { kind: mine.kind.clone(), data, deleted: mine.deleted, conflicts }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn e(v: Value) -> Envelope {
        Envelope::new("t", v)
    }

    #[test]
    fn different_fields_merge_cleanly() {
        let b = e(json!({"a": 1, "b": 1}));
        let m = merge3(Some(&b), &e(json!({"a": 2, "b": 1})), &e(json!({"a": 1, "b": 3})));
        assert_eq!(m.data, json!({"a": 2, "b": 3}));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn same_field_keeps_both_versions() {
        let b = e(json!({"a": 1}));
        let m = merge3(Some(&b), &e(json!({"a": 2})), &e(json!({"a": 3})));
        assert_eq!(m.data, json!({"a": 2}));
        assert_eq!(m.conflicts, vec![FieldConflict { field: "a".into(), mine: json!(2), theirs: json!(3) }]);
    }

    #[test]
    fn equal_edits_are_not_a_conflict() {
        let b = e(json!({"a": 1}));
        let m = merge3(Some(&b), &e(json!({"a": 2})), &e(json!({"a": 2})));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn field_removed_on_one_side_stays_removed() {
        let b = e(json!({"a": 1, "b": 2}));
        let m = merge3(Some(&b), &e(json!({"a": 1})), &e(json!({"a": 1, "b": 2})));
        assert_eq!(m.data, json!({"a": 1}));
    }

    #[test]
    fn delete_against_edit_keeps_the_edit_and_flags_it() {
        let b = e(json!({"a": 1}));
        let mut del = b.clone();
        del.deleted = true;
        for (mine, theirs) in [(&del, &e(json!({"a": 2}))), (&e(json!({"a": 2})), &del)] {
            let m = merge3(Some(&b), mine, theirs);
            assert!(!m.deleted);
            assert_eq!(m.data, json!({"a": 2}));
            assert_eq!(m.conflicts[0].field, FIELD_DELETED);
        }
    }

    #[test]
    fn delete_against_unchanged_deletes() {
        let b = e(json!({"a": 1}));
        let mut del = b.clone();
        del.deleted = true;
        assert!(merge3(Some(&b), &del, &b).deleted);
        assert!(merge3(Some(&b), &b, &del).deleted);
    }

    #[test]
    fn both_deleted_stays_deleted() {
        let b = e(json!({"a": 1}));
        let mut del = b.clone();
        del.deleted = true;
        let m = merge3(Some(&b), &del, &del);
        assert!(m.deleted && m.conflicts.is_empty());
    }

    #[test]
    fn existing_conflicts_survive_a_later_merge() {
        let b = e(json!({"a": 1}));
        let first = merge3(Some(&b), &e(json!({"a": 2})), &e(json!({"a": 3})));
        let again = merge3(Some(&b), &first, &e(json!({"a": 1})));
        assert_eq!(again.conflicts.len(), 1);
    }

    #[test]
    fn resolve_applies_choice() {
        let b = e(json!({"a": 1}));
        let mut m = merge3(Some(&b), &e(json!({"a": 2})), &e(json!({"a": 3})));
        m.resolve("a", true);
        assert_eq!(m.data, json!({"a": 3}));
        assert!(m.conflicts.is_empty());
    }

    #[test]
    fn resolve_deleted_choice() {
        let b = e(json!({"a": 1}));
        let mut del = b.clone();
        del.deleted = true;
        let mut m = merge3(Some(&b), &del, &e(json!({"a": 2})));
        m.resolve(FIELD_DELETED, false); // my version: deleted
        assert!(m.deleted);
    }

    #[test]
    fn non_object_data_conflicts_as_a_whole() {
        let m = merge3(Some(&e(json!(1))), &e(json!(2)), &e(json!(3)));
        assert_eq!(m.conflicts[0].field, "");
    }
}
