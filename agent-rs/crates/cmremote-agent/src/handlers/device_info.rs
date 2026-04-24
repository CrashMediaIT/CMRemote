// Source: CMRemote, clean-room implementation.

//! Device-information hub handler (slice R3).

use cmremote_platform::DeviceInfoProvider;
use cmremote_wire::ConnectionInfo;

/// Handle `TriggerHeartbeat`: collect a `DeviceSnapshot` and serialise
/// it as a JSON object to return as the completion result.
pub fn handle_trigger_heartbeat(
    info: &ConnectionInfo,
    provider: &dyn DeviceInfoProvider,
) -> Result<serde_json::Value, String> {
    let mut snap = provider.snapshot().map_err(|e| e.to_string())?;
    snap.device_id = info.device_id.to_string();
    snap.organization_id = info.organization_id.clone().unwrap_or_default();
    serde_json::to_value(snap).map_err(|e| e.to_string())
}
