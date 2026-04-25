// Source: CMRemote, clean-room implementation.

//! Windows session info (slice R7.n.3).
//!
//! Reports the Windows session topology the agent is running in.
//! Critical for input injection: `SendInput` silently inserts
//! **zero** events when called from a process attached to **Session
//! 0** (the non-interactive session that hosts services and the
//! `LocalSystem` account on every modern Windows). The Windows
//! input drivers from slice R7.n.2 don't surface that condition on
//! their own — they ask the kernel to inject and trust the count
//! the kernel returns. This module lets the agent runtime check
//! up-front whether injection is even possible, so a
//! desktop-control session can be refused with a structured error
//! instead of silently swallowing every event.
//!
//! ## Concepts
//!
//! - **Session ID** — a `u32` the kernel assigns to every logon.
//!   Session 0 is the non-interactive services session;
//!   Session 1+ are interactive (console / RDP).
//! - **Console session** — the session currently bound to the
//!   physical console. RDP detaches the console session id away
//!   from the original console; locking the workstation does not.
//!   Returned by `WTSGetActiveConsoleSessionId()`.
//! - **Current session** — the session this process is in,
//!   returned by `ProcessIdToSessionId(GetCurrentProcessId())`.
//!
//! ## Threading
//!
//! Both Win32 calls are O(1) and lock-free; the snapshot is built
//! synchronously and held by value (no resources to release).
//!
//! ## Security
//!
//! The reported session ids are non-sensitive integers (every
//! process on the host can already learn its own session id; there
//! is no privilege boundary to leak across). Errors carry only
//! opaque `HRESULT 0x...` codes — never process names, OS message
//! text, or any operator-supplied identifier.

use thiserror::Error;

#[cfg(target_os = "windows")]
use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::GetCurrentProcessId;

/// Sentinel returned by [`WTSGetActiveConsoleSessionId`] when no
/// session is currently attached to the physical console (e.g.
/// during a fast-user-switch transition). Surfaced from the
/// snapshot as `console_session_id == None`.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
const NO_CONSOLE_SESSION: u32 = 0xFFFF_FFFF;

/// Errors surfaced by the session-info APIs.
#[derive(Debug, Error)]
pub enum WindowsSessionError {
    /// A Win32 call returned a failed `HRESULT`. The opaque
    /// `HRESULT 0x...` is the only payload — implementation MUST
    /// NOT include the OS message text (which on some Windows
    /// versions can interpolate process / handle names).
    #[error("Windows session info I/O error: {0}")]
    Io(String),
}

/// Snapshot of the Windows session topology at the moment of
/// capture.
///
/// The snapshot is plain data — taking it again yields a fresh
/// view (the console session id can change at any time when an
/// operator switches between RDP and the console).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowsSessionInfo {
    /// Session id of the calling process. `0` means Session 0
    /// (services / `LocalSystem`); `1+` is interactive.
    pub current_session_id: u32,
    /// Session id of the physical console, or `None` when the
    /// console is currently detached (the underlying Win32 sentinel
    /// is `0xFFFFFFFF`).
    pub console_session_id: Option<u32>,
}

impl WindowsSessionInfo {
    /// Build a snapshot from raw component values. Mainly useful
    /// for tests; production code calls
    /// [`WindowsSessionInfo::current`].
    pub const fn new(current_session_id: u32, console_session_id: Option<u32>) -> Self {
        Self {
            current_session_id,
            console_session_id,
        }
    }

    /// Capture the live session topology of the calling process.
    #[cfg(target_os = "windows")]
    pub fn current() -> Result<Self, WindowsSessionError> {
        // SAFETY: GetCurrentProcessId has no failure mode and
        // touches no caller-owned memory.
        let pid = unsafe { GetCurrentProcessId() };
        let mut sid: u32 = 0;
        // SAFETY: `&mut sid` is a valid `*mut u32`; the call writes
        // exactly one `u32` on success and leaves it unchanged on
        // failure (we do not read on failure).
        unsafe { ProcessIdToSessionId(pid, &mut sid as *mut u32) }.map_err(|e| {
            WindowsSessionError::Io(format!("ProcessIdToSessionId: {}", os_code(&e)))
        })?;

        // SAFETY: WTSGetActiveConsoleSessionId has no failure mode;
        // it returns `0xFFFFFFFF` when no console session is
        // currently attached.
        let console_raw = unsafe { WTSGetActiveConsoleSessionId() };
        let console = if console_raw == NO_CONSOLE_SESSION {
            None
        } else {
            Some(console_raw)
        };

        Ok(Self {
            current_session_id: sid,
            console_session_id: console,
        })
    }

    /// Non-Windows stub so the symbol is callable cross-platform
    /// for tests / docs builds. Always returns `Io`.
    #[cfg(not(target_os = "windows"))]
    pub fn current() -> Result<Self, WindowsSessionError> {
        Err(WindowsSessionError::Io(
            "WindowsSessionInfo::current is only available on Windows".into(),
        ))
    }

    /// True iff the calling process is in Session 0 — the
    /// non-interactive session reserved for services on every
    /// modern Windows. `SendInput` silently injects nothing in
    /// this state because Session 0 has no interactive desktop.
    pub const fn is_session_zero(&self) -> bool {
        self.current_session_id == 0
    }

    /// True iff the calling process shares its session id with the
    /// currently active console session — i.e. the operator is
    /// physically (or via RDP) looking at the same desktop the
    /// agent would inject into.
    ///
    /// Returns `false` when the console is currently detached
    /// (e.g. mid-fast-user-switch) or when the calling process is
    /// in a different session (e.g. an RDP session running
    /// alongside an idle console).
    pub const fn is_in_console_session(&self) -> bool {
        match self.console_session_id {
            Some(c) => c == self.current_session_id,
            None => false,
        }
    }

    /// Convenience: true iff input injection is *expected* to be
    /// observable to a human operator at the host.
    ///
    /// Conservative — returns `false` for Session 0 even if the
    /// console session id happens to also be 0 (which can occur on
    /// pre-Vista Windows, but is meaningless on every supported
    /// host). The agent runtime should refuse desktop-control
    /// requests when this returns `false`, surfacing a structured
    /// "agent is running as a service / no interactive desktop
    /// attached" failure.
    pub const fn can_inject_input(&self) -> bool {
        !self.is_session_zero() && self.is_in_console_session()
    }
}

#[cfg(target_os = "windows")]
fn os_code(e: &windows::core::Error) -> String {
    // Mirror the formatter used by the input / capture modules so
    // every Windows error in this crate has a consistent shape.
    format!("HRESULT 0x{:08X}", e.code().0 as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_zero_is_session_zero() {
        let s = WindowsSessionInfo::new(0, Some(1));
        assert!(s.is_session_zero());
        assert!(!s.is_in_console_session());
        assert!(!s.can_inject_input());
    }

    #[test]
    fn matching_console_session_can_inject() {
        let s = WindowsSessionInfo::new(2, Some(2));
        assert!(!s.is_session_zero());
        assert!(s.is_in_console_session());
        assert!(s.can_inject_input());
    }

    #[test]
    fn mismatched_console_session_cannot_inject() {
        // RDP session 3 while the physical console belongs to
        // session 1 — agent in session 3 must not inject because
        // the operator is looking at a different desktop.
        let s = WindowsSessionInfo::new(3, Some(1));
        assert!(!s.is_in_console_session());
        assert!(!s.can_inject_input());
    }

    #[test]
    fn detached_console_cannot_inject() {
        // Mid-fast-user-switch: WTSGetActiveConsoleSessionId
        // returns the 0xFFFFFFFF sentinel.
        let s = WindowsSessionInfo::new(2, None);
        assert!(!s.is_in_console_session());
        assert!(!s.can_inject_input());
    }

    #[test]
    fn session_zero_with_console_zero_still_refuses_injection() {
        // Defensive: the conservative `can_inject_input` rule
        // refuses Session 0 outright, even on the (extinct)
        // pre-Vista hosts where the console could legitimately
        // share session id 0.
        let s = WindowsSessionInfo::new(0, Some(0));
        assert!(s.is_session_zero());
        assert!(s.is_in_console_session());
        assert!(!s.can_inject_input());
    }

    #[test]
    fn snapshot_round_trips_through_clone_and_eq() {
        let s = WindowsSessionInfo::new(7, Some(1));
        let s2 = s;
        assert_eq!(s, s2);
        // Hash stability is part of the contract — pin it via the
        // derived implementation.
        let mut set = std::collections::HashSet::new();
        set.insert(s);
        assert!(set.contains(&s2));
    }

    #[test]
    fn error_messages_carry_only_opaque_hresult() {
        let e = WindowsSessionError::Io("HRESULT 0x80004005".into());
        let s = e.to_string();
        assert!(s.contains("HRESULT 0x80004005"), "{s}");
        assert!(s.contains("session info"), "{s}");
        // Must not echo any operator-supplied path-shaped value
        // (the constructor accepts an arbitrary string, so this
        // pins the *current* call site as the only producer).
        assert!(!s.contains("\\\\"), "{s}");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn current_returns_structured_error_on_non_windows() {
        let r = WindowsSessionInfo::current();
        assert!(matches!(r, Err(WindowsSessionError::Io(_))));
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "calls live Win32 APIs; requires a real Windows host"]
    fn current_returns_a_snapshot_on_real_windows() {
        let s = WindowsSessionInfo::current().expect("snapshot");
        // Smoke: the helpers must not panic on the live snapshot.
        let _ = s.is_session_zero();
        let _ = s.is_in_console_session();
        let _ = s.can_inject_input();
    }
}
