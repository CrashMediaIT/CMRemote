// Source: CMRemote, clean-room implementation.

//! Device-information hub handler (slice R3).

use std::sync::Arc;

use cmremote_platform::DeviceInfoProvider;
use cmremote_wire::ConnectionInfo;

/// Handle `TriggerHeartbeat`: collect a `DeviceSnapshot` and serialise
/// it as a JSON object to return as the completion result.
///
/// `provider.snapshot()` performs blocking work (a 100 ms CPU sample
/// and a `df` subprocess on Linux), so we hand it off to
/// `tokio::task::spawn_blocking` to keep the dispatch task off the
/// async runtime worker pool.
pub async fn handle_trigger_heartbeat(
    info: &ConnectionInfo,
    provider: Arc<dyn DeviceInfoProvider>,
) -> Result<serde_json::Value, String> {
    let device_id = info.device_id.to_string();
    let organization_id = info.organization_id.clone().unwrap_or_default();

    let snap = tokio::task::spawn_blocking(move || provider.snapshot())
        .await
        .map_err(|e| format!("snapshot task join failed: {e}"))?
        .map_err(|e| e.to_string())?;

    let snap = cmremote_platform::DeviceSnapshot {
        device_id,
        organization_id,
        ..snap
    };
    serde_json::to_value(snap).map_err(|e| e.to_string())
}
