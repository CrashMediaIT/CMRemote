// Source: CMRemote, clean-room implementation.

//! Package-manager provider trait and helpers (slice R6).
//!
//! Re-derived from `Agent/Interfaces/IPackageProvider.cs` and the
//! `Shared.PackageManager` helpers in the .NET agent. The shapes are
//! re-authored from the spec; nothing is copied verbatim.
//!
//! ## Security contract
//!
//! Every implementation must respect three rules so that the agent
//! never becomes a remote-code-execution channel:
//!
//! 1. **No wire executables.** The agent re-resolves the actual command
//!    line locally — `choco.exe`, `msiexec.exe`, … — from the OS, never
//!    from a string carried on the wire.
//! 2. **Allow-list every operator-supplied identifier.** Use
//!    [`is_safe_chocolatey_package_id`] / [`is_safe_chocolatey_version`]
//!    / [`is_safe_msi_file_name`] before passing anything to a child
//!    process.
//! 3. **Verify what you download.** Before invoking `msiexec`, an
//!    implementation MUST re-hash the downloaded bytes with
//!    [`compute_sha256_hex`] and check the OLE2 magic with
//!    [`is_msi_magic_bytes`]; mismatch is a hard refusal, not a
//!    warning.
//!
//! Slice R6 ships the trait, the safety helpers, the
//! [`NotSupportedPackageProvider`] stub, and the
//! [`CompositePackageProvider`] router. The concrete fetch + install
//! providers (Chocolatey, MSI, Executable) land alongside the
//! signed-build pipeline (slice R8) so the agent never sees an
//! unsigned variant; the trait + safety helpers are public so those
//! providers compose without further wire/contract churn.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cmremote_wire::{PackageInstallRequest, PackageInstallResult, PackageProvider};
use sha2::{Digest, Sha256};

use crate::HostOs;

pub mod chocolatey;
pub mod download;
pub mod executable;
pub mod msi;
pub mod process;

pub use chocolatey::ChocolateyPackageProvider;
pub use download::{
    ArtifactDownloader, DownloadError, DownloadRequest, DownloadedArtifact, RejectingDownloader,
};
pub use executable::ExecutablePackageProvider;
pub use msi::UploadedMsiPackageProvider;
pub use process::{ProcessCommand, ProcessOutcome, ProcessRunner, TokioProcessRunner};

/// First eight bytes of every OLE2 / Compound File Binary (CFB)
/// document, of which an MSI is one. The .NET `MsiFileValidator`
/// uses the same eight-byte signature.
pub const OLE2_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// Maximum length of a Chocolatey package id we accept on the wire.
/// Chocolatey itself caps at 100; we mirror that.
pub const MAX_CHOCO_ID_LEN: usize = 100;

/// Maximum length of a Chocolatey version string. Generous enough for
/// the longest semver-with-build-metadata strings observed in the wild.
pub const MAX_CHOCO_VERSION_LEN: usize = 64;

/// Maximum length of an MSI filename leaf.
pub const MAX_MSI_FILENAME_LEN: usize = 255;

/// Chocolatey exit codes that mean "the package operation reached the
/// desired post-state, even if the OS reported a non-zero code". Mirrors
/// `Shared/PackageManager/ChocolateyOutputParser.cs::SuccessfulExitCodes`.
///
/// * `0`       — clean success.
/// * `1605`    — `ERROR_UNKNOWN_PRODUCT` on uninstall (already gone).
/// * `1614`    — `ERROR_PRODUCT_UNINSTALLED` (alternate uninstall path).
/// * `1641`    — installer initiated a reboot.
/// * `3010`    — installer requires a reboot to finish.
pub const CHOCOLATEY_SUCCESS_EXIT_CODES: &[i32] = &[0, 1605, 1614, 1641, 3010];

/// Returns `true` when `code` is in [`CHOCOLATEY_SUCCESS_EXIT_CODES`].
pub fn is_chocolatey_success_exit_code(code: i32) -> bool {
    CHOCOLATEY_SUCCESS_EXIT_CODES.contains(&code)
}

/// Allow-list check for Chocolatey package ids. Accepts only ASCII
/// alphanumerics, `.`, `-`, and `_`, capped at [`MAX_CHOCO_ID_LEN`].
pub fn is_safe_chocolatey_package_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_CHOCO_ID_LEN {
        return false;
    }
    id.bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'_')
}

/// Allow-list check for Chocolatey version strings. Accepts only ASCII
/// alphanumerics, `.`, `-`, and `+`, capped at [`MAX_CHOCO_VERSION_LEN`].
pub fn is_safe_chocolatey_version(version: &str) -> bool {
    if version.is_empty() || version.len() > MAX_CHOCO_VERSION_LEN {
        return false;
    }
    version
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-' || c == b'+')
}

/// Allow-list check for an operator-supplied MSI filename leaf.
///
/// Rejects path separators, NUL, control characters, and anything
/// longer than [`MAX_MSI_FILENAME_LEN`]. The filename is used **only**
/// as the leaf name the agent writes to before invoking `msiexec`; the
/// directory is chosen by the agent from a fixed cache path.
pub fn is_safe_msi_file_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_MSI_FILENAME_LEN {
        return false;
    }
    // No path separators, no NUL, no control characters. Reserved
    // Windows characters (`<>:"/\|?*`) are rejected as well so the
    // same allow-list applies on every host.
    if name.bytes().any(|c| {
        c == b'/'
            || c == b'\\'
            || c == 0
            || c < 0x20
            || matches!(c, b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*')
    }) {
        return false;
    }
    // Reject explicit relative-path components.
    if name == "." || name == ".." {
        return false;
    }
    true
}

/// `true` when `bytes` starts with the OLE2 magic signature
/// ([`OLE2_MAGIC`]). Required check before invoking `msiexec`.
pub fn is_msi_magic_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= OLE2_MAGIC.len() && bytes[..OLE2_MAGIC.len()] == OLE2_MAGIC
}

/// Compute the lowercase hex SHA-256 digest of `bytes`. Used by the
/// MSI installer to verify what it just downloaded against the
/// `MsiSha256` field on the wire.
pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode_lower(&digest)
}

fn hex_encode_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Constant-time string equality for hex-encoded SHA-256 digests.
/// Lower-cases both inputs first so a server that emits uppercase
/// hex still matches.
pub fn ct_eq_hex(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in expected.bytes().zip(actual.bytes()) {
        diff |= a.to_ascii_lowercase() ^ b.to_ascii_lowercase();
    }
    diff == 0
}

/// Per-OS / per-provider implementation of the agent-side install /
/// uninstall workflow. Implementations must not panic — failures are
/// surfaced via the returned [`PackageInstallResult`].
#[async_trait]
pub trait PackageProviderHandler: Send + Sync {
    /// `true` when this handler can service `request` on the current
    /// host (e.g. `choco.exe` exists on PATH for the Chocolatey
    /// handler). Pure check; never spawns a child process.
    fn can_handle(&self, request: &PackageInstallRequest) -> bool;

    /// Execute the request. Implementations must respect the security
    /// contract documented at the module level.
    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult;
}

/// Handler returned by [`CompositePackageProvider`] for any provider
/// the current OS does not implement. Always reports a structured
/// failure with `success=false`, `exit_code=-1`, and an operator-facing
/// message naming the missing provider — never panics.
pub struct NotSupportedPackageProvider {
    host_os: HostOs,
}

impl NotSupportedPackageProvider {
    /// Construct a provider that names the supplied OS in its error
    /// message. Use [`Self::for_current_host`] for the typical case.
    pub fn new(host_os: HostOs) -> Self {
        Self { host_os }
    }

    /// Construct a provider for the host the agent is running on.
    pub fn for_current_host() -> Self {
        Self::new(HostOs::current())
    }
}

impl Default for NotSupportedPackageProvider {
    fn default() -> Self {
        Self::for_current_host()
    }
}

#[async_trait]
impl PackageProviderHandler for NotSupportedPackageProvider {
    fn can_handle(&self, _request: &PackageInstallRequest) -> bool {
        false
    }

    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
        PackageInstallResult::failed(
            request.job_id.clone(),
            format!(
                "Provider {:?} is not supported on {:?}.",
                request.provider, self.host_os
            ),
        )
    }
}

/// Routes a [`PackageInstallRequest`] to the registered handler for
/// the request's [`PackageProvider`]. Mirrors the .NET
/// `CompositePackageProvider` so the hub keeps a single dependency.
///
/// Handlers are looked up by `PackageProvider`; if none is registered
/// the request is answered with [`NotSupportedPackageProvider`] so the
/// operator gets a structured failure rather than a hung job.
pub struct CompositePackageProvider {
    handlers: HashMap<PackageProvider, Box<dyn PackageProviderHandler>>,
    fallback: Box<dyn PackageProviderHandler>,
}

impl CompositePackageProvider {
    /// Build a composite with no handlers registered. Every request
    /// will route to the not-supported fallback. Use [`Self::register`]
    /// to add handlers.
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
            fallback: Box::new(NotSupportedPackageProvider::for_current_host()),
        }
    }

    /// Register `handler` as the implementation for `provider`. Panics
    /// in debug mode if `provider` is [`PackageProvider::Unknown`] —
    /// `Unknown` is reserved for "wire payload was malformed" and must
    /// never have a handler bound to it.
    pub fn register(
        &mut self,
        provider: PackageProvider,
        handler: Box<dyn PackageProviderHandler>,
    ) -> &mut Self {
        debug_assert_ne!(
            provider,
            PackageProvider::Unknown,
            "PackageProvider::Unknown must never have a handler"
        );
        self.handlers.insert(provider, handler);
        self
    }

    /// Register the default per-OS handler set for slice R6:
    ///
    /// * On Windows: [`ChocolateyPackageProvider`] (always),
    ///   [`UploadedMsiPackageProvider`] and [`ExecutablePackageProvider`]
    ///   (gated on a downloader being supplied).
    /// * On every other OS: nothing — install jobs continue to fall
    ///   through to the structured "not supported" fallback. This is
    ///   intentional: `choco.exe` and `msiexec.exe` are Windows-only
    ///   and the `Executable` lane assumes the agent runs on the
    ///   target host as a privileged installer.
    ///
    /// `cache_dir` is the directory the MSI / Executable providers
    /// stage downloads into. The runtime is responsible for creating
    /// it (chmod 0700 on Unix). `server_host` is the URL the
    /// downloader hits to fetch artifacts; it comes from
    /// `ConnectionInfo::host`. `downloader` is the HTTPS client used
    /// for fetches; pass an [`Arc<RejectingDownloader>`] until the
    /// real reqwest-based client is wired (the providers' download
    /// step then refuses with a clean "this agent is not configured
    /// to download package artifacts" message).
    pub fn register_default_handlers(
        &mut self,
        cache_dir: std::path::PathBuf,
        server_host: Option<String>,
        downloader: Arc<dyn ArtifactDownloader>,
    ) -> &mut Self {
        // Chocolatey ships unconditionally on every OS. The
        // [`StdChocolateyEnvironment`] returns `None` for `choco.exe`
        // on non-Windows hosts so `can_handle` is `false`; the
        // execute path returns a structured failure for the same
        // reason. This lets the registration code stay
        // platform-agnostic.
        self.register(
            PackageProvider::Chocolatey,
            Box::new(ChocolateyPackageProvider::new()),
        );

        let msi_env = Arc::new(msi::StdMsiEnvironment::new(
            cache_dir.clone(),
            server_host.clone(),
        ));
        self.register(
            PackageProvider::UploadedMsi,
            Box::new(UploadedMsiPackageProvider::new_with(
                msi_env,
                Arc::new(TokioProcessRunner),
                downloader.clone(),
                msi::MSI_DOWNLOAD_TIMEOUT,
                msi::MSIEXEC_TIMEOUT,
                msi::MAX_MSI_BYTES,
            )),
        );

        let exe_env = Arc::new(executable::StdExecutableEnvironment::new(
            cache_dir,
            server_host,
        ));
        self.register(
            PackageProvider::Executable,
            Box::new(ExecutablePackageProvider::new_with(
                exe_env,
                Arc::new(TokioProcessRunner),
                downloader,
                executable::EXECUTABLE_DOWNLOAD_TIMEOUT,
                executable::EXECUTABLE_TIMEOUT,
                executable::MAX_EXECUTABLE_BYTES,
            )),
        );

        self
    }
}

impl Default for CompositePackageProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PackageProviderHandler for CompositePackageProvider {
    fn can_handle(&self, request: &PackageInstallRequest) -> bool {
        if request.provider == PackageProvider::Unknown {
            return false;
        }
        self.handlers
            .get(&request.provider)
            .is_some_and(|h| h.can_handle(request))
    }

    async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
        if request.provider == PackageProvider::Unknown {
            return PackageInstallResult::failed(
                request.job_id.clone(),
                "Unknown package provider; refusing to dispatch.",
            );
        }
        match self.handlers.get(&request.provider) {
            Some(handler) => handler.execute(request).await,
            None => self.fallback.execute(request).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use cmremote_wire::PackageInstallAction;

    use super::*;

    #[test]
    fn safe_choco_id_accepts_typical_packages() {
        assert!(is_safe_chocolatey_package_id("googlechrome"));
        assert!(is_safe_chocolatey_package_id("vscode.install"));
        assert!(is_safe_chocolatey_package_id("dotnet-sdk-8"));
        assert!(is_safe_chocolatey_package_id("Some_App_2"));
    }

    #[test]
    fn safe_choco_id_rejects_shell_metacharacters() {
        for bad in &[
            "",
            "; rm -rf /",
            "pkg|ls",
            "pkg`whoami`",
            "pkg$(id)",
            "pkg&calc",
            "pkg with space",
            "pkg\\evil",
            "pkg/../../etc",
            "pkg\nnewline",
        ] {
            assert!(
                !is_safe_chocolatey_package_id(bad),
                "expected reject: {bad:?}"
            );
        }
    }

    #[test]
    fn safe_choco_id_enforces_length_cap() {
        let too_long = "a".repeat(MAX_CHOCO_ID_LEN + 1);
        assert!(!is_safe_chocolatey_package_id(&too_long));
        let just_right = "a".repeat(MAX_CHOCO_ID_LEN);
        assert!(is_safe_chocolatey_package_id(&just_right));
    }

    #[test]
    fn safe_choco_version_accepts_typical_versions() {
        assert!(is_safe_chocolatey_version("1.2.3"));
        assert!(is_safe_chocolatey_version("1.2.3-rc.1"));
        assert!(is_safe_chocolatey_version("1.2.3+build.7"));
    }

    #[test]
    fn safe_choco_version_rejects_metacharacters() {
        for bad in &["", "1.2;ls", "1 2 3", "1.2.3$"] {
            assert!(!is_safe_chocolatey_version(bad), "expected reject: {bad:?}");
        }
    }

    #[test]
    fn safe_msi_file_name_accepts_normal_names() {
        assert!(is_safe_msi_file_name("setup.msi"));
        assert!(is_safe_msi_file_name("Acme.Tools-1.2.3.msi"));
    }

    #[test]
    fn safe_msi_file_name_rejects_traversal_and_specials() {
        for bad in &[
            "",
            ".",
            "..",
            "../etc/passwd",
            "..\\windows",
            "subdir/file.msi",
            "C:\\evil.msi",
            "name\0bad.msi",
            "name\nbad.msi",
            "name|bad.msi",
            "name<bad.msi",
            "name>bad.msi",
            "name:bad.msi",
            "name\"bad.msi",
            "name?bad.msi",
            "name*bad.msi",
        ] {
            assert!(!is_safe_msi_file_name(bad), "expected reject: {bad:?}");
        }
    }

    #[test]
    fn msi_magic_match_accepts_ole2_header() {
        let mut bytes = Vec::from(OLE2_MAGIC);
        bytes.extend_from_slice(&[0u8; 32]);
        assert!(is_msi_magic_bytes(&bytes));
    }

    #[test]
    fn msi_magic_rejects_short_or_wrong_prefix() {
        assert!(!is_msi_magic_bytes(&[]));
        assert!(!is_msi_magic_bytes(&[0xD0, 0xCF, 0x11, 0xE0]));
        assert!(!is_msi_magic_bytes(b"MZthis-is-an-exe"));
        // Same length as the magic but different bytes.
        assert!(!is_msi_magic_bytes(&[0u8; 8]));
    }

    #[test]
    fn sha256_hex_known_vector() {
        // Empty-string SHA-256 vector from FIPS-180-4.
        assert_eq!(
            compute_sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc" vector.
        assert_eq!(
            compute_sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn ct_eq_hex_is_case_insensitive_and_length_strict() {
        let lower = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let upper = lower.to_ascii_uppercase();
        assert!(ct_eq_hex(lower, &upper));
        assert!(!ct_eq_hex(lower, &lower[..63]));
        assert!(!ct_eq_hex(lower, &format!("{lower}0")));
        // One-bit difference.
        let mut munged = String::from(lower);
        munged.replace_range(0..1, "f");
        assert!(!ct_eq_hex(lower, &munged));
    }

    #[test]
    fn chocolatey_success_exit_codes_match_spec() {
        for code in CHOCOLATEY_SUCCESS_EXIT_CODES {
            assert!(is_chocolatey_success_exit_code(*code));
        }
        for code in &[1, 2, 1603, 1612, 9999, -1] {
            assert!(!is_chocolatey_success_exit_code(*code));
        }
    }

    fn req(provider: PackageProvider) -> PackageInstallRequest {
        PackageInstallRequest {
            job_id: "j".into(),
            provider,
            action: PackageInstallAction::Install,
            package_identifier: "pkg".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn not_supported_is_structured_failure_not_panic() {
        let p = NotSupportedPackageProvider::new(HostOs::Linux);
        let r = p.execute(&req(PackageProvider::Chocolatey)).await;
        assert!(!r.success);
        assert_eq!(r.exit_code, -1);
        let msg = r.error_message.unwrap();
        assert!(msg.contains("Chocolatey"), "{msg}");
        assert!(msg.contains("Linux"), "{msg}");
    }

    #[tokio::test]
    async fn composite_unknown_provider_short_circuits() {
        let composite = CompositePackageProvider::new();
        let r = composite.execute(&req(PackageProvider::Unknown)).await;
        assert!(!r.success);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains("unknown"));
    }

    #[tokio::test]
    async fn composite_with_no_handler_falls_back_to_not_supported() {
        let composite = CompositePackageProvider::new();
        let r = composite.execute(&req(PackageProvider::Chocolatey)).await;
        assert!(!r.success);
        assert!(r
            .error_message
            .as_deref()
            .unwrap_or("")
            .contains("not supported"));
    }

    /// Test handler: succeeds for matching provider, records the
    /// invocation count.
    struct FakeHandler {
        provider: PackageProvider,
        invocations: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl PackageProviderHandler for FakeHandler {
        fn can_handle(&self, request: &PackageInstallRequest) -> bool {
            request.provider == self.provider
        }

        async fn execute(&self, request: &PackageInstallRequest) -> PackageInstallResult {
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            PackageInstallResult {
                job_id: request.job_id.clone(),
                success: true,
                exit_code: 0,
                duration_ms: 1,
                stdout_tail: None,
                stderr_tail: None,
                error_message: None,
            }
        }
    }

    #[tokio::test]
    async fn composite_routes_to_registered_handler() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut composite = CompositePackageProvider::new();
        composite.register(
            PackageProvider::Chocolatey,
            Box::new(FakeHandler {
                provider: PackageProvider::Chocolatey,
                invocations: counter.clone(),
            }),
        );

        let r = composite.execute(&req(PackageProvider::Chocolatey)).await;
        assert!(r.success);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Different provider falls back to not-supported, not the
        // registered Chocolatey handler.
        let r2 = composite.execute(&req(PackageProvider::UploadedMsi)).await;
        assert!(!r2.success);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn composite_can_handle_respects_handler_predicate() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut composite = CompositePackageProvider::new();
        composite.register(
            PackageProvider::Chocolatey,
            Box::new(FakeHandler {
                provider: PackageProvider::Chocolatey,
                invocations: counter,
            }),
        );

        assert!(composite.can_handle(&req(PackageProvider::Chocolatey)));
        assert!(!composite.can_handle(&req(PackageProvider::UploadedMsi)));
        assert!(!composite.can_handle(&req(PackageProvider::Unknown)));
    }
}
