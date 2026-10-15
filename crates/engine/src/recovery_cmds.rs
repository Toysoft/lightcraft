//! Recovery operations use the command registry for CLI, control and MCP callers.
//! JSON transport is capped separately from the binary browser checkpoint adapter.
use crate::cmd::{CommandSpec, always};
use crate::{EngineError, Result, Session};
use serde_json::{Value, json};
const JSON_LIMIT: usize = 8 * 1024 * 1024;
fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams { cmd: cmd.into(), msg: msg.into() }
}
fn bytes(params: &Value, cmd: &str) -> Result<Vec<u8>> {
    let array = params.get("bytes").and_then(Value::as_array).ok_or_else(|| bad(cmd, "bytes must be an array of integers"))?;
    if array.len() > JSON_LIMIT {
        return Err(bad(cmd, "JSON recovery transport exceeds 8 MiB; use the binary adapter"));
    }
    array.iter().map(|v| v.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| bad(cmd, "bytes must be integers from 0 to 255"))).collect()
}
fn capture(s: &mut Session, p: &Value) -> Result<Value> {
    let bytes = s.craft_capture(p.get("ui").cloned().unwrap_or(Value::Null)).map_err(EngineError::Other)?;
    if bytes.len() > JSON_LIMIT {
        return Err(bad("session.recovery.capture", "JSON recovery transport exceeds 8 MiB; use the binary adapter"));
    }
    Ok(json!({"bytes":bytes}))
}
fn inspect(_: &mut Session, p: &Value) -> Result<Value> {
    Session::craft_ui(&bytes(p, "session.recovery.inspect")?).map(|ui| json!({"ui":ui})).map_err(EngineError::Other)
}
fn restore(s: &mut Session, p: &Value) -> Result<Value> {
    if !s.catalog.is_empty() {
        return Err(bad("session.recovery.restore", "close current work before restoring"));
    }
    s.craft_restore(&bytes(p, "session.recovery.restore")?).map(|ui| json!({"ui":ui})).map_err(EngineError::Other)
}
pub(crate) fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: "session.recovery.capture",
            label: "Capture editing session",
            menu: &[],
            shortcut: None,
            params: r#"{ui?:object}"#,
            enabled: always,
            run: capture,
            journal: false,
            explicit_targets: false,
        },
        CommandSpec {
            id: "session.recovery.inspect",
            label: "Inspect editing session",
            menu: &[],
            shortcut: None,
            params: r#"{bytes:[u8] <= 8 MiB}"#,
            enabled: always,
            run: inspect,
            journal: false,
            explicit_targets: false,
        },
        CommandSpec {
            id: "session.recovery.restore",
            label: "Restore editing session",
            menu: &[],
            shortcut: None,
            params: r#"{bytes:[u8] <= 8 MiB; requires empty session}"#,
            enabled: always,
            run: restore,
            journal: false,
            explicit_targets: false,
        },
    ]
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_commands_return_errors_without_mutating_work() {
        let mut s = Session::new();
        for p in [
            Value::Null,
            json!({}),
            json!({"bytes":[-1]}),
            json!({"bytes":[256]}),
            json!({"bytes":[1.5]}),
            json!({"bytes":["0"]}),
            json!({"bytes":[0]}),
        ] {
            assert!(inspect(&mut s, &p).is_err());
            assert!(restore(&mut s, &p).is_err());
        }
    }
    #[test]
    fn capture_inspect_restore_are_registered() {
        let mut s = Session::new();
        let p = json!({"ui":{"panel":"test"}});
        let captured = s.execute("session.recovery.capture", &p).unwrap();
        assert_eq!(s.execute("session.recovery.inspect", &captured).unwrap()["ui"], p["ui"]);
        let mut restored = Session::new();
        assert_eq!(restored.execute("session.recovery.restore", &captured).unwrap()["ui"], p["ui"]);
    }
}
