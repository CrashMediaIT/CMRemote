// Source: CMRemote, clean-room implementation.

//! Hub handlers for the four desktop-transport methods (slice R7 —
//! initial wire-surface + dispatch routing).
//!
//! Each handler:
//!
//! 1. Decodes the [`HubInvocation::arguments`] vector into the matching
//!    request DTO from [`cmremote_wire::desktop`].
//! 2. Forwards the request to the registered
//!    [`DesktopTransportProvider`].
//! 3. Returns the JSON-encoded [`DesktopTransportResult`] for the
//!    completion frame.
//!
//! Decoding failures are translated into a structured failure result
//! (not a `not_implemented` completion) so the operator sees a clear
//! "malformed request" message in the audit trail.

use cmremote_platform::desktop::DesktopTransportProvider;
use cmremote_wire::{
    ChangeWindowsSessionRequest, DesktopTransportResult, HubInvocation, InvokeCtrlAltDelRequest,
    RemoteControlSessionRequest, RestartScreenCasterRequest,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Decode a SignalR-style positional `arguments` array into a single
/// request DTO. The .NET hub sends each method argument as a JSON
/// object inside `arguments[0]`; for parameter-less methods the array
/// is empty and we use [`Default::default`].
fn decode_single_arg<T: DeserializeOwned + Default>(inv: &HubInvocation) -> Result<T, String> {
    match inv.arguments.first() {
        None => Ok(T::default()),
        Some(Value::Null) => Ok(T::default()),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| format!("invalid arguments for {}: {e}", inv.target)),
    }
}

fn result_to_json(r: &DesktopTransportResult) -> Result<Value, String> {
    serde_json::to_value(r).map_err(|e| format!("failed to serialise result: {e}"))
}

/// Handler for `RemoteControl(sessionId, accessKey, …)`.
pub async fn handle_remote_control(
    inv: &HubInvocation,
    provider: &dyn DesktopTransportProvider,
) -> Result<Value, String> {
    let req: RemoteControlSessionRequest = match decode_single_arg(inv) {
        Ok(r) => r,
        Err(e) => {
            // Surface as a structured failure rather than a wire-level
            // error so the dispatcher's audit trail records the bad
            // payload.
            let r = DesktopTransportResult::failed(String::new(), e);
            return result_to_json(&r);
        }
    };
    let r = provider.remote_control(&req).await;
    result_to_json(&r)
}

/// Handler for `RestartScreenCaster(viewerIds, sessionId, …)`.
pub async fn handle_restart_screen_caster(
    inv: &HubInvocation,
    provider: &dyn DesktopTransportProvider,
) -> Result<Value, String> {
    let req: RestartScreenCasterRequest = match decode_single_arg(inv) {
        Ok(r) => r,
        Err(e) => {
            let r = DesktopTransportResult::failed(String::new(), e);
            return result_to_json(&r);
        }
    };
    let r = provider.restart_screen_caster(&req).await;
    result_to_json(&r)
}

/// Handler for `ChangeWindowsSession(viewerConnectionId, sessionId, …,
/// targetSessionId)`.
pub async fn handle_change_windows_session(
    inv: &HubInvocation,
    provider: &dyn DesktopTransportProvider,
) -> Result<Value, String> {
    let req: ChangeWindowsSessionRequest = match decode_single_arg(inv) {
        Ok(r) => r,
        Err(e) => {
            let r = DesktopTransportResult::failed(String::new(), e);
            return result_to_json(&r);
        }
    };
    let r = provider.change_windows_session(&req).await;
    result_to_json(&r)
}

/// Handler for `InvokeCtrlAltDel()` — no arguments.
pub async fn handle_invoke_ctrl_alt_del(
    _inv: &HubInvocation,
    provider: &dyn DesktopTransportProvider,
) -> Result<Value, String> {
    let r = provider.invoke_ctrl_alt_del(&InvokeCtrlAltDelRequest).await;
    result_to_json(&r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmremote_platform::desktop::NotSupportedDesktopTransport;
    use cmremote_platform::HostOs;
    use cmremote_wire::HubMessageKind;
    use serde_json::json;

    fn invocation(target: &str, args: Vec<Value>) -> HubInvocation {
        HubInvocation {
            kind: HubMessageKind::Invocation as u8,
            invocation_id: Some("inv-1".into()),
            target: target.into(),
            arguments: args,
        }
    }

    #[tokio::test]
    async fn remote_control_routes_to_provider_and_serialises_result() {
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        let inv = invocation(
            "RemoteControl",
            vec![json!({
                "SessionId": "sess-1",
                "AccessKey": "ak",
                "UserConnectionId": "v",
                "RequesterName": "Alice",
                "OrgName": "Acme",
                "OrgId": "org",
            })],
        );
        let v = handle_remote_control(&inv, &provider).await.unwrap();
        assert_eq!(v["SessionId"], "sess-1");
        assert_eq!(v["Success"], false);
        let msg = v["ErrorMessage"].as_str().unwrap();
        assert!(msg.contains("RemoteControl"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
        // Sensitive: access key must never appear in the completion
        // payload.
        assert!(!msg.contains("ak"), "access_key leaked into result: {msg}");
    }

    #[tokio::test]
    async fn restart_screen_caster_routes_and_includes_session_id() {
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        let inv = invocation(
            "RestartScreenCaster",
            vec![json!({
                "ViewerIds": ["v1", "v2"],
                "SessionId": "sess-2",
                "AccessKey": "ak",
                "UserConnectionId": "u",
                "RequesterName": "r",
                "OrgName": "o",
                "OrgId": "i",
            })],
        );
        let v = handle_restart_screen_caster(&inv, &provider).await.unwrap();
        assert_eq!(v["SessionId"], "sess-2");
        assert_eq!(v["Success"], false);
    }

    #[tokio::test]
    async fn change_windows_session_carries_target_session_id() {
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        let inv = invocation(
            "ChangeWindowsSession",
            vec![json!({
                "ViewerConnectionId": "v",
                "SessionId": "sess-3",
                "AccessKey": "ak",
                "UserConnectionId": "u",
                "RequesterName": "r",
                "OrgName": "o",
                "OrgId": "i",
                "TargetSessionId": 7,
            })],
        );
        let v = handle_change_windows_session(&inv, &provider)
            .await
            .unwrap();
        assert_eq!(v["SessionId"], "sess-3");
        assert_eq!(v["Success"], false);
    }

    #[tokio::test]
    async fn invoke_ctrl_alt_del_takes_no_arguments() {
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        let inv = invocation("InvokeCtrlAltDel", vec![]);
        let v = handle_invoke_ctrl_alt_del(&inv, &provider).await.unwrap();
        assert_eq!(v["Success"], false);
        // No session id in the request; result echoes empty string.
        assert_eq!(v["SessionId"], "");
        assert!(v["ErrorMessage"]
            .as_str()
            .unwrap()
            .contains("InvokeCtrlAltDel"));
    }

    #[tokio::test]
    async fn malformed_arguments_become_structured_failure() {
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        // SessionId is required as a string — pass a number instead.
        let inv = invocation(
            "RemoteControl",
            vec![json!({
                "SessionId": 12345,
                "AccessKey": "ak",
                "UserConnectionId": "u",
                "RequesterName": "r",
                "OrgName": "o",
                "OrgId": "i",
            })],
        );
        let v = handle_remote_control(&inv, &provider).await.unwrap();
        assert_eq!(v["Success"], false);
        assert!(v["ErrorMessage"]
            .as_str()
            .unwrap()
            .contains("invalid arguments"));
    }

    #[tokio::test]
    async fn missing_arguments_default_to_empty_request() {
        // The .NET hub never sends an empty arguments array for
        // RemoteControl, but a malformed peer might. Default values
        // mean we synthesise a `success=false` result with the empty
        // session id and the not-supported message — never panic.
        let provider = NotSupportedDesktopTransport::new(HostOs::Linux);
        let inv = invocation("RemoteControl", vec![]);
        let v = handle_remote_control(&inv, &provider).await.unwrap();
        assert_eq!(v["Success"], false);
        assert_eq!(v["SessionId"], "");
    }
}
