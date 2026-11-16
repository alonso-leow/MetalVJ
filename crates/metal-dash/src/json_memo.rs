//! A parse memo for the shell's JSON reads (2026-10-09, measured with
//! `crate::perf`): the read cache (`shell::reads`) hands every panel the same
//! reply bytes for a quarter second, and each panel then parsed them again —
//! `/author/models/state` alone is ~360 KB and was parsed several times a
//! frame. Here a body is hashed and its parsed `Value` reused while the same
//! bytes keep coming back. Per thread, small, cleared when it fills.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use serde_json::Value;

/// Bodies smaller than this parse faster than they hash and clone.
const MEMO_MIN: usize = 4096;
/// Entries kept before the memo starts over.
const MEMO_CAP: usize = 48;

thread_local! {
    static MEMO: RefCell<HashMap<(usize, u64), Value>> = RefCell::new(HashMap::new());
    static SHARED: RefCell<HashMap<(usize, u64), Rc<Value>>> = RefCell::new(HashMap::new());
}

fn key_of(bytes: &[u8]) -> (usize, u64) {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    (bytes.len(), h.finish())
}

/// [`parse`] without the clone: the same bytes answer the same `Rc`, so a
/// panel that only reads a large reply (the model manifest) never copies it.
pub fn parse_shared(bytes: &[u8]) -> Option<Rc<Value>> {
    let key = key_of(bytes);
    if let Some(v) = SHARED.with(|m| m.borrow().get(&key).cloned()) {
        return Some(v);
    }
    let v = Rc::new(serde_json::from_slice::<Value>(bytes).ok()?);
    SHARED.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MEMO_CAP {
            m.clear();
        }
        m.insert(key, Rc::clone(&v));
    });
    Some(v)
}

/// `serde_json::from_slice`, memoised for large bodies.
pub fn parse(bytes: &[u8]) -> Result<Value, serde_json::Error> {
    if bytes.len() < MEMO_MIN {
        return serde_json::from_slice(bytes);
    }
    let key = key_of(bytes);
    if let Some(v) = MEMO.with(|m| m.borrow().get(&key).cloned()) {
        return Ok(v);
    }
    let v: Value = serde_json::from_slice(bytes)?;
    MEMO.with(|m| {
        let mut m = m.borrow_mut();
        if m.len() >= MEMO_CAP {
            m.clear();
        }
        m.insert(key, v.clone());
    });
    Ok(v)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_large_body_parses_the_same_twice() {
        let big = serde_json::json!({ "models": (0..400).map(|i| serde_json::json!({ "id": i })).collect::<Vec<_>>() });
        let bytes = serde_json::to_vec(&big).unwrap();
        assert!(bytes.len() >= super::MEMO_MIN);
        assert_eq!(super::parse(&bytes).unwrap(), big);
        assert_eq!(super::parse(&bytes).unwrap(), big);
    }
}
