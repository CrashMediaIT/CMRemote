// Source: CMRemote, clean-room implementation.

//! On-host consent-prompt trait surface (slice R7.h).
//!
//! Before any concrete [`super::input`] driver applies a viewer's
//! mouse / keyboard / clipboard event, the desktop transport MUST
//! obtain on-host consent from the user actually sitting at the
//! controlled machine. This module ships the trait every concrete
//! prompter implements, the request / decision DTOs that flow
//! through it, and a fail-closed [`DenyAllConsentPrompter`] default
//! the runtime can use until a per-OS UI ([`super::input`] driver,
//! GTK / Win32 / Cocoa) is wired in.
//!
//! ## Layering
//!
//! Same "trait first, real driver later" pattern used by R7.c
//! ([`super::media`]) and R7.h ([`super::input`]):
//!
//! 1. The wire layer never carries a consent decision — consent is
//!    a purely host-local affair, contractually outside the .NET
//!    server's trust boundary. The DTOs here only describe the
//!    request the prompter renders and the decision it returns.
//! 2. [`ConsentPrompter`] is a `Send + Sync` async trait so a real
//!    prompter can `await` user input on a UI event loop without
//!    blocking the agent's tokio worker pool.
//! 3. The default [`DenyAllConsentPrompter`] denies every request —
//!    a desktop session reaches a host with no consent UI configured
//!    and is refused, *not* silently allowed.
//!
//! ## Security contract
//!
//! Every prompter MUST:
//!
//! 1. **Sanitise operator-supplied strings before rendering them.**
//!    [`ConsentRequest::sanitised`] runs the same
//!    [`super::guards::validate_operator_string`] check the desktop
//!    transport's envelope guards apply, so a hostile org name with
//!    bidi-override characters cannot disguise itself in the prompt.
//!    Concrete prompters MUST construct requests via
//!    [`ConsentRequest::sanitised`] (or perform the equivalent
//!    check themselves) — never display a raw inbound string.
//! 2. **Fail closed on any error.** A prompter that cannot reach
//!    the host UI (no display server, locked screen, prompter
//!    crash) MUST surface [`ConsentDecision::Denied`] — never a
//!    permissive default.
//! 3. **Honour the request timeout.** A prompt that is left on
//!    screen indefinitely is a denial-of-service vector against
//!    legitimate users; the prompter MUST surface
//!    [`ConsentDecision::Timeout`] when [`ConsentRequest::timeout`]
//!    elapses without a response.
//! 4. **Log only the sanitised summary.** A prompter MUST NOT log
//!    the inbound raw string; the audit trail records the
//!    sanitised form so a hostile name cannot smuggle terminal
//!    escapes into log files.

use std::time::Duration;

use async_trait::async_trait;

use cmremote_wire::{DesktopTransportResult, RemoteControlSessionRequest};

use super::guards::{validate_operator_string, GuardRejection};
#[cfg(test)]
use super::guards::MAX_OPERATOR_STRING_LEN;

/// Default time the host UI waits for the operator to answer the
/// consent prompt before the prompter MUST surface
/// [`ConsentDecision::Timeout`]. Mirrors the .NET implementation's
/// 30-second budget; concrete prompters can override per-request via
/// [`ConsentRequest::timeout`].
pub const DEFAULT_CONSENT_TIMEOUT: Duration = Duration::from_secs(30);

/// Inbound consent request after sanitisation.
///
/// Construct via [`ConsentRequest::sanitised`] — the constructor
/// runs every operator-supplied string through the same security
/// contract the desktop transport's envelope guards apply, so a
/// concrete prompter never has to repeat the check before rendering
/// the prompt on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentRequest {
    /// Canonical lowercase-UUID session id. The prompter renders
    /// this on screen so the operator can correlate the prompt with
    /// the audit-log entry the server emits.
    pub session_id: String,
    /// Display name of the viewer requesting access. Already
    /// sanitised — safe to render verbatim.
    pub requester_name: String,
    /// Display name of the viewer's organisation. Already
    /// sanitised — safe to render verbatim.
    pub org_name: String,
    /// SignalR connection id of the viewer (the .NET hub's
    /// `viewerConnectionId` parameter). Useful for the audit trail
    /// when a single viewer opens multiple parallel sessions.
    pub viewer_connection_id: String,
    /// How long the prompter SHOULD wait for the operator to answer
    /// before surfacing [`ConsentDecision::Timeout`]. Defaults to
    /// [`DEFAULT_CONSENT_TIMEOUT`] when constructed via
    /// [`ConsentRequest::sanitised`].
    pub timeout: Duration,
}

impl ConsentRequest {
    /// Build a prompt-ready [`ConsentRequest`] from the raw fields
    /// the desktop transport carries on the wire. Returns
    /// `Err(field_message)` if any field fails the same operator-
    /// string check the [`super::guards`] envelope applies (over
    /// [`MAX_OPERATOR_STRING_LEN`] bytes, contains a control
    /// character / NUL / DEL, or contains a Unicode bidi-override
    /// code point).
    pub fn sanitised(
        session_id: impl Into<String>,
        requester_name: impl Into<String>,
        org_name: impl Into<String>,
        viewer_connection_id: impl Into<String>,
    ) -> Result<Self, String> {
        let session_id = session_id.into();
        let requester_name = requester_name.into();
        let org_name = org_name.into();
        let viewer_connection_id = viewer_connection_id.into();

        // session_id is already required to be a canonical
        // lowercase UUID by the envelope guards, but the consent
        // module is also reachable from tests / future callers, so
        // re-validate it with the operator-string contract — that
        // rejects the same hostile bytes (control chars, bidi
        // overrides) and the length cap is well above 36.
        validate_operator_string("session_id", &session_id)?;
        validate_operator_string("requester_name", &requester_name)?;
        validate_operator_string("org_name", &org_name)?;
        validate_operator_string("viewer_connection_id", &viewer_connection_id)?;

        Ok(Self {
            session_id,
            requester_name,
            org_name,
            viewer_connection_id,
            timeout: DEFAULT_CONSENT_TIMEOUT,
        })
    }

    /// Replace the timeout while keeping the other (already
    /// sanitised) fields. Returns `Err` if the supplied timeout is
    /// zero — a zero-timeout prompt is indistinguishable from "no
    /// prompt at all" and would silently deny every session.
    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, String> {
        if timeout.is_zero() {
            return Err("timeout must be greater than zero".into());
        }
        self.timeout = timeout;
        Ok(self)
    }
}

/// Outcome the prompter returns to the desktop transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentDecision {
    /// The operator approved the session — input injection MAY
    /// begin until the session ends.
    Granted,
    /// The operator denied the session, OR the prompter could not
    /// reach the host UI (in which case the contract requires
    /// fail-closed denial). `reason` is operator-facing and MUST
    /// NOT contain raw operator-supplied strings.
    Denied {
        /// Short, fixed-shape reason string suitable for the audit
        /// log and for surfacing to the viewer ("operator declined",
        /// "no host UI available", …).
        reason: String,
    },
    /// The prompt was on screen for [`ConsentRequest::timeout`]
    /// without a response.
    Timeout,
}

impl ConsentDecision {
    /// `true` only when the decision is [`ConsentDecision::Granted`].
    pub fn is_granted(&self) -> bool {
        matches!(self, ConsentDecision::Granted)
    }
}

/// Renders the on-host consent prompt and returns the operator's
/// decision.
#[async_trait]
pub trait ConsentPrompter: Send + Sync {
    /// Display the prompt described by `request` and block until
    /// the operator answers, the prompt is cancelled, or
    /// `request.timeout` elapses. Implementations MUST NOT panic
    /// under any input — see the module-level security contract.
    async fn request_consent(&self, request: &ConsentRequest) -> ConsentDecision;
}

/// Default prompter the runtime registers when no concrete UI driver
/// is available. Always returns [`ConsentDecision::Denied`] with a
/// fixed `"no consent UI is configured on this host"` reason — fail-
/// closed, by contract.
#[derive(Debug, Default)]
pub struct DenyAllConsentPrompter;

#[async_trait]
impl ConsentPrompter for DenyAllConsentPrompter {
    async fn request_consent(&self, _request: &ConsentRequest) -> ConsentDecision {
        ConsentDecision::Denied {
            reason: "no consent UI is configured on this host".into(),
        }
    }
}

/// Test-only prompter that auto-approves every request. Compiled
/// in only under `cfg(test)` so it cannot accidentally ship in a
/// release binary.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct AutoApproveConsentPrompter;

#[cfg(test)]
#[async_trait]
impl ConsentPrompter for AutoApproveConsentPrompter {
    async fn request_consent(&self, _request: &ConsentRequest) -> ConsentDecision {
        ConsentDecision::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SESSION_ID: &str = "11111111-2222-3333-4444-555555555555";

    fn ok_request() -> ConsentRequest {
        ConsentRequest::sanitised(VALID_SESSION_ID, "Alice", "Acme", "viewer-1").unwrap()
    }

    #[tokio::test]
    async fn deny_all_prompter_denies_every_request_with_a_fixed_reason() {
        let p = DenyAllConsentPrompter;
        let r = ok_request();
        let d = p.request_consent(&r).await;
        match d {
            ConsentDecision::Denied { reason } => {
                assert!(reason.contains("no consent UI"), "{reason}");
                // Reason MUST NOT echo any operator-supplied string.
                assert!(!reason.contains("Alice"));
                assert!(!reason.contains("Acme"));
                assert!(!reason.contains(VALID_SESSION_ID));
            }
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn auto_approve_prompter_grants_every_request() {
        let p = AutoApproveConsentPrompter;
        let d = p.request_consent(&ok_request()).await;
        assert!(d.is_granted());
    }

    #[test]
    fn sanitised_constructor_accepts_a_clean_request_with_default_timeout() {
        let r = ok_request();
        assert_eq!(r.session_id, VALID_SESSION_ID);
        assert_eq!(r.requester_name, "Alice");
        assert_eq!(r.org_name, "Acme");
        assert_eq!(r.viewer_connection_id, "viewer-1");
        assert_eq!(r.timeout, DEFAULT_CONSENT_TIMEOUT);
    }

    #[test]
    fn sanitised_constructor_refuses_bidi_override_in_org_name() {
        // U+202E RIGHT-TO-LEFT OVERRIDE — the "Trojan Source"
        // attack vector the guards module documents.
        let err = ConsentRequest::sanitised(VALID_SESSION_ID, "Alice", "Acme\u{202E}", "viewer-1")
            .unwrap_err();
        assert!(err.contains("org_name"));
        assert!(err.contains("bidi-override"));
    }

    #[test]
    fn sanitised_constructor_refuses_embedded_nul() {
        let err =
            ConsentRequest::sanitised(VALID_SESSION_ID, "Alice\0", "Acme", "viewer-1").unwrap_err();
        assert!(err.contains("requester_name"));
        assert!(err.contains("non-printable"));
    }

    #[test]
    fn sanitised_constructor_refuses_over_length_org_name() {
        let huge = "x".repeat(MAX_OPERATOR_STRING_LEN + 1);
        let err =
            ConsentRequest::sanitised(VALID_SESSION_ID, "Alice", huge, "viewer-1").unwrap_err();
        assert!(err.contains("org_name"));
        assert!(err.contains("limit"));
    }

    #[test]
    fn sanitised_constructor_refuses_empty_required_fields() {
        let err = ConsentRequest::sanitised(VALID_SESSION_ID, "", "Acme", "viewer-1").unwrap_err();
        assert!(err.contains("requester_name"));
        assert!(err.contains("empty"));
    }

    #[test]
    fn sanitised_constructor_refuses_control_in_viewer_connection_id() {
        let err = ConsentRequest::sanitised(
            VALID_SESSION_ID,
            "Alice",
            "Acme",
            "viewer-1\u{0007}", // BEL
        )
        .unwrap_err();
        assert!(err.contains("viewer_connection_id"));
        assert!(err.contains("non-printable"));
    }

    #[test]
    fn with_timeout_overrides_the_default_and_refuses_zero() {
        let r = ok_request().with_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(r.timeout, Duration::from_secs(5));
        let err = ok_request().with_timeout(Duration::ZERO).unwrap_err();
        assert!(err.contains("greater than zero"));
    }

    #[test]
    fn consent_decision_is_granted_only_for_granted() {
        assert!(ConsentDecision::Granted.is_granted());
        assert!(!ConsentDecision::Timeout.is_granted());
        assert!(!ConsentDecision::Denied { reason: "x".into() }.is_granted());
    }

    /// Trait-object safety check — the prompter must be storable
    /// behind `Box<dyn …>` so the runtime can swap the default for
    /// a per-OS UI implementation at startup.
    #[test]
    fn prompter_is_object_safe() {
        let _p: Box<dyn ConsentPrompter> = Box::new(DenyAllConsentPrompter);
    }
}
