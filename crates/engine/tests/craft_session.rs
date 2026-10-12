#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use lightcraft_engine::{Session, demo};
use serde_json::json;
#[test]
fn craft_session_keeps_catalog_selection_filter_and_undo_redo() {
    let mut s = Session::new();
    demo::load(&mut s);
    let id = s.active().unwrap();
    s.execute("photo.rate", &json!({"rating":4})).unwrap();
    s.execute("photo.rate", &json!({"rating":5})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    let bytes = s.craft_capture(json!({"panel":"develop"})).unwrap();
    let mut r = Session::new();
    assert_eq!(r.craft_restore(&bytes).unwrap(), json!({"panel":"develop"}));
    assert_eq!(r.catalog.len(), s.catalog.len());
    assert_eq!(r.active(), Some(id));
    assert_eq!(r.catalog.photo(id).unwrap().rating, 4);
    r.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(r.catalog.photo(id).unwrap().rating, 5);
    r.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(r.catalog.photo(id).unwrap().rating, 4);
    assert!(r.craft_restore(&bytes[..20]).is_err());
    assert_eq!(r.catalog.len(), s.catalog.len());
}

#[test]
fn recovery_inspection_rejects_wrong_app_and_malformed_envelopes() {
    for bytes in [b"".as_slice(), b"CRAFTS01", b"CRAFTS01\xff\xff\xff\xff"] {
        assert!(Session::craft_ui(bytes).is_err());
    }
    let meta = br#"{"app":"unrelated","schema":1,"ui":{}}"#;
    let mut bytes = b"CRAFTS01".to_vec();
    bytes.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    bytes.extend_from_slice(meta);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    assert!(Session::craft_ui(&bytes).is_err());
}
