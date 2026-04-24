// Source: CMRemote, clean-room implementation.

//! WebSocket-over-TLS connect, header construction, and Sec-WebSocket-Protocol
//! negotiation.
//!
//! The HTTP-level shape is pinned by `docs/wire-protocol.md` ➜
//! *Authentication and identity*. Every upgrade carries:
//!
//! * `Authorization: Bearer <organization-token>` (omitted when the
//!   on-disk config has not been issued one yet).
//! * `X-Device-Id: <uuid-v4>`.
//! * `X-Protocol-Version: 1`.
//! * `X-Server-Verification: <token>` on every connect after the first.
//!
//! Plain `ws://` is rejected at this layer — the `wss://` floor in
//! the spec is non-negotiable, even on `localhost`.

use cmremote_wire::{ConnectionInfo, HubProtocol};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Uri};

/// Wire-protocol version this build of the agent speaks.
///
/// Mirrors `docs/wire-protocol.md` ➜ *Versioning*: bumping this is a
/// roadmap-tracked decision, not an editorial change.
pub const PROTOCOL_VERSION: u8 = 1;

/// Path component of the agent hub on every CMRemote server.
pub const AGENT_HUB_PATH: &str = "/hubs/agent";

/// Errors raised when constructing the upgrade request.
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// The configured `Host` could not be parsed as a URL.
    #[error("invalid Host URL `{host}`: {reason}")]
    InvalidHost {
        /// The offending host string as it appeared in the config.
        host: String,
        /// Human-readable parse error.
        reason: String,
    },

    /// The agent refused to use a non-`wss://` transport.
    ///
    /// Pinned by `docs/wire-protocol.md` ➜ *Transport* — this is a
    /// security floor, not a configurable knob.
    #[error("only wss:// is supported; got `{0}`")]
    InsecureScheme(String),

    /// A header value rejected by the HTTP library (e.g. non-ASCII).
    #[error("invalid header value: {0}")]
    InvalidHeader(String),

    /// Underlying WebSocket / TLS error from the upgrade handshake.
    #[error("websocket upgrade failed: {0}")]
    Upgrade(Box<tokio_tungstenite::tungstenite::Error>),

    /// The server returned a `Sec-WebSocket-Protocol` value the agent
    /// did not advertise. Per spec the agent must close with code
    /// `1002` in this case.
    #[error("server selected unsupported sub-protocol `{0}`")]
    UnsupportedSubProtocol(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for ConnectError {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        ConnectError::Upgrade(Box::new(e))
    }
}

/// Build the WebSocket-upgrade `Request` for a given configuration
/// + preferred hub-protocol encoding.
///
/// The `wss://` scheme is enforced here; the caller does not need to
/// re-check it.
pub fn build_request(
    info: &ConnectionInfo,
    preferred: HubProtocol,
) -> Result<Request, ConnectError> {
    let host = info
        .normalized_host()
        .ok_or_else(|| ConnectError::InvalidHost {
            host: String::new(),
            reason: "Host is empty".to_owned(),
        })?;

    let uri: Uri = ws_uri(&host)?;
    let mut req = uri
        .into_client_request()
        .map_err(|e| ConnectError::InvalidHost {
            host: host.clone(),
            reason: e.to_string(),
        })?;

    let headers = req.headers_mut();

    // Per-request identity headers. The bearer token is optional —
    // the spec allows a freshly-deployed agent to enrol without one
    // and the server hands one back on first connect.
    if let Some(token) = info.organization_token.as_deref() {
        let value = HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ConnectError::InvalidHeader("Authorization".to_owned()))?;
        headers.insert(http_name("authorization"), value);
    }

    headers.insert(
        http_name("x-device-id"),
        HeaderValue::from_str(&info.device_id)
            .map_err(|_| ConnectError::InvalidHeader("X-Device-Id".to_owned()))?,
    );

    headers.insert(
        http_name("x-protocol-version"),
        HeaderValue::from_str(&PROTOCOL_VERSION.to_string())
            .expect("ascii integer is always a valid header value"),
    );

    if let Some(svt) = info.server_verification_token.as_deref() {
        let value = HeaderValue::from_str(svt)
            .map_err(|_| ConnectError::InvalidHeader("X-Server-Verification".to_owned()))?;
        headers.insert(http_name("x-server-verification"), value);
    }

    // Sec-WebSocket-Protocol carries the SignalR sub-protocol(s) we are
    // willing to speak. Putting the preferred encoding first matches
    // the spec's production-default policy; the server's reply tells
    // us which one was actually selected and that's enforced on the
    // response side via [`negotiate_subprotocol`].
    let sub_protocols = match preferred {
        HubProtocol::Messagepack => "messagepack, json",
        HubProtocol::Json => "json, messagepack",
    };
    debug_assert!(
        sub_protocols.contains("json") && sub_protocols.contains("messagepack"),
        "both encodings must be advertised so the server can pick"
    );
    headers.insert(
        http_name("sec-websocket-protocol"),
        HeaderValue::from_static(sub_protocols),
    );

    Ok(req)
}

/// Construct the canonical `wss://<host>/hubs/agent` URI from a
/// configured `Host`.
fn ws_uri(host: &str) -> Result<Uri, ConnectError> {
    // Parse with `Uri::try_from` so we get a proper error on garbage
    // rather than a silent fallthrough.
    let parsed: Uri =
        host.parse()
            .map_err(|e: tokio_tungstenite::tungstenite::http::uri::InvalidUri| {
                ConnectError::InvalidHost {
                    host: host.to_owned(),
                    reason: e.to_string(),
                }
            })?;

    // Translate https → wss. Plain http → reject up front. The agent
    // never opens a `ws://` socket, even on localhost.
    let scheme = parsed
        .scheme_str()
        .ok_or_else(|| ConnectError::InvalidHost {
            host: host.to_owned(),
            reason: "missing scheme".to_owned(),
        })?;
    let ws_scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "wss",
        other => return Err(ConnectError::InsecureScheme(other.to_owned())),
    };

    let authority = parsed
        .authority()
        .ok_or_else(|| ConnectError::InvalidHost {
            host: host.to_owned(),
            reason: "missing authority".to_owned(),
        })?
        .as_str()
        .to_owned();

    // Compose path: any operator-supplied trailing path is preserved
    // (matches what the .NET client did) so a deployment behind a
    // reverse-proxy sub-path keeps working. The agent hub path is
    // appended verbatim.
    let mut path = parsed.path().trim_end_matches('/').to_owned();
    path.push_str(AGENT_HUB_PATH);

    let uri_string = if let Some(query) = parsed.query() {
        format!("{ws_scheme}://{authority}{path}?{query}")
    } else {
        format!("{ws_scheme}://{authority}{path}")
    };

    uri_string
        .parse::<Uri>()
        .map_err(|e| ConnectError::InvalidHost {
            host: host.to_owned(),
            reason: e.to_string(),
        })
}

fn http_name(name: &'static str) -> HeaderName {
    // All names are static lowercase ASCII; `from_static` panics
    // only on malformed input, which is a programmer error here.
    HeaderName::from_static(name)
}

/// Validate the `Sec-WebSocket-Protocol` header the server returned
/// against the encodings the agent advertised, returning the
/// encoding that will actually be used for the lifetime of the
/// connection.
pub fn negotiate_subprotocol(
    server_response: Option<&HeaderValue>,
) -> Result<HubProtocol, ConnectError> {
    let value = match server_response {
        Some(v) => v,
        // An absent header means the server didn't pick one. SignalR
        // treats that as "json" by default; we honour that to keep
        // older servers working but log it at the call-site.
        None => return Ok(HubProtocol::Json),
    };

    let s = value
        .to_str()
        .map_err(|_| ConnectError::UnsupportedSubProtocol("<non-ascii>".to_owned()))?;
    match s.trim() {
        "json" => Ok(HubProtocol::Json),
        "messagepack" => Ok(HubProtocol::Messagepack),
        other => Err(ConnectError::UnsupportedSubProtocol(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info_with_host(host: &str) -> ConnectionInfo {
        ConnectionInfo {
            device_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            host: Some(host.to_owned()),
            organization_id: Some("org-1".to_owned()),
            server_verification_token: None,
            organization_token: None,
        }
    }

    #[test]
    fn https_host_translates_to_wss_with_hub_path() {
        let info = info_with_host("https://cmremote.example.com");
        let req = build_request(&info, HubProtocol::Json).unwrap();
        let uri = req.uri();
        assert_eq!(uri.scheme_str(), Some("wss"));
        assert_eq!(uri.host(), Some("cmremote.example.com"));
        assert_eq!(uri.path(), AGENT_HUB_PATH);
    }

    #[test]
    fn host_subpath_is_preserved_in_front_of_hub_path() {
        let info = info_with_host("https://proxy.example.com/cmremote");
        let req = build_request(&info, HubProtocol::Json).unwrap();
        assert_eq!(req.uri().path(), "/cmremote/hubs/agent");
    }

    #[test]
    fn wss_scheme_is_accepted_directly() {
        let info = info_with_host("wss://cmremote.example.com");
        let req = build_request(&info, HubProtocol::Json).unwrap();
        assert_eq!(req.uri().scheme_str(), Some("wss"));
    }

    #[test]
    fn http_scheme_is_rejected() {
        let info = info_with_host("http://cmremote.example.com");
        let err = build_request(&info, HubProtocol::Json).unwrap_err();
        assert!(matches!(err, ConnectError::InsecureScheme(_)));
    }

    #[test]
    fn ws_scheme_is_rejected() {
        let info = info_with_host("ws://cmremote.example.com");
        let err = build_request(&info, HubProtocol::Json).unwrap_err();
        assert!(matches!(err, ConnectError::InsecureScheme(_)));
    }

    #[test]
    fn missing_host_is_rejected() {
        let info = ConnectionInfo {
            device_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            host: None,
            organization_id: Some("org-1".to_owned()),
            server_verification_token: None,
            organization_token: None,
        };
        let err = build_request(&info, HubProtocol::Json).unwrap_err();
        assert!(matches!(err, ConnectError::InvalidHost { .. }));
    }

    #[test]
    fn identity_headers_are_attached() {
        let mut info = info_with_host("https://cmremote.example.com");
        info.organization_token = Some("ot-secret".into());
        info.server_verification_token = Some("svt-value".into());
        let req = build_request(&info, HubProtocol::Messagepack).unwrap();
        let h = req.headers();
        assert_eq!(
            h.get("authorization").unwrap().to_str().unwrap(),
            "Bearer ot-secret"
        );
        assert_eq!(
            h.get("x-device-id").unwrap().to_str().unwrap(),
            "00000000-0000-0000-0000-000000000001"
        );
        assert_eq!(h.get("x-protocol-version").unwrap().to_str().unwrap(), "1");
        assert_eq!(
            h.get("x-server-verification").unwrap().to_str().unwrap(),
            "svt-value"
        );
        assert_eq!(
            h.get("sec-websocket-protocol").unwrap().to_str().unwrap(),
            "messagepack, json"
        );
    }

    #[test]
    fn no_authorization_header_when_org_token_absent() {
        let info = info_with_host("https://cmremote.example.com");
        let req = build_request(&info, HubProtocol::Json).unwrap();
        assert!(req.headers().get("authorization").is_none());
    }

    #[test]
    fn json_preferred_orders_subprotocols_correctly() {
        let info = info_with_host("https://cmremote.example.com");
        let req = build_request(&info, HubProtocol::Json).unwrap();
        assert_eq!(
            req.headers()
                .get("sec-websocket-protocol")
                .unwrap()
                .to_str()
                .unwrap(),
            "json, messagepack"
        );
    }

    #[test]
    fn negotiate_picks_messagepack_when_server_says_so() {
        let v = HeaderValue::from_static("messagepack");
        let p = negotiate_subprotocol(Some(&v)).unwrap();
        assert_eq!(p, HubProtocol::Messagepack);
    }

    #[test]
    fn negotiate_picks_json_when_server_says_so() {
        let v = HeaderValue::from_static("json");
        let p = negotiate_subprotocol(Some(&v)).unwrap();
        assert_eq!(p, HubProtocol::Json);
    }

    #[test]
    fn negotiate_defaults_to_json_when_header_absent() {
        let p = negotiate_subprotocol(None).unwrap();
        assert_eq!(p, HubProtocol::Json);
    }

    #[test]
    fn negotiate_rejects_unknown_value() {
        let v = HeaderValue::from_static("protobuf");
        let err = negotiate_subprotocol(Some(&v)).unwrap_err();
        assert!(matches!(err, ConnectError::UnsupportedSubProtocol(_)));
    }
}
