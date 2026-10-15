#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
use lightcraft_engine::Session;
use serde_json::{Value, json};
#[test]
fn recovery_commands_reject_malformed_transport_and_round_trip() {
    let mut s = Session::new();
    for p in
        [Value::Null, json!({}), json!({"bytes":[-1]}), json!({"bytes":[256]}), json!({"bytes":[1.5]}), json!({"bytes":["0"]}), json!({"bytes":[0]})]
    {
        assert!(s.execute("session.recovery.inspect", &p).is_err());
        assert!(s.execute("session.recovery.restore", &p).is_err());
    }
    let p = json!({"ui":{"panel":"test"}});
    let captured = s.execute("session.recovery.capture", &p).unwrap();
    let p = captured.clone();
    assert_eq!(s.execute("session.recovery.inspect", &p).unwrap()["ui"], json!({"panel":"test"}));
    let mut restored = Session::new();
    assert_eq!(restored.execute("session.recovery.restore", &p).unwrap()["ui"], json!({"panel":"test"}));
}
