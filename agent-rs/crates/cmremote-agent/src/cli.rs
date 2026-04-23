// Source: CMRemote, clean-room implementation.

//! Minimal command-line argument parser.
//!
//! We deliberately avoid pulling in `clap` for the R0 scaffold: the
//! agent only needs a handful of flags, every dependency we add ends
//! up on every managed endpoint, and clap's macros pull in syn /
//! proc-macro2 transitively. If the surface grows past ~6 flags we'll
//! revisit.

use std::ffi::OsString;

/// Parsed command-line arguments.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CliArgs {
    /// Override the path to `ConnectionInfo.json`.
    pub config_path: Option<String>,

    /// Override `Host`.
    pub host: Option<String>,

    /// Override `OrganizationID`.
    pub organization: Option<String>,

    /// Override `DeviceID`.
    pub device: Option<String>,

    /// `--help` was requested; the binary should print usage and exit.
    pub help: bool,

    /// `--version` was requested; the binary should print the build
    /// version and exit.
    pub version: bool,
}

/// Errors produced by the CLI parser.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// An option was given without the value it requires.
    #[error("flag `{0}` requires a value")]
    MissingValue(String),

    /// An unrecognised argument was supplied.
    #[error("unknown argument: `{0}`")]
    Unknown(String),
}

impl CliArgs {
    /// Parse from any iterator of `OsString`. The first element is
    /// assumed to be the program name and is ignored, matching the
    /// shape of `std::env::args_os()`.
    pub fn parse<I>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut out = CliArgs::default();
        let mut iter = args.into_iter();
        let _program = iter.next();

        while let Some(raw) = iter.next() {
            let arg = raw.to_string_lossy().into_owned();
            // Normalise: strip leading dashes, lowercase. This matches
            // the legacy .NET agent's tolerant key handling.
            let key = arg.trim_start_matches('-').to_ascii_lowercase();

            match key.as_str() {
                "help" | "h" | "?" => out.help = true,
                "version" | "v" => out.version = true,
                "config" => out.config_path = Some(take_value(&arg, &mut iter)?),
                "host" => out.host = Some(take_value(&arg, &mut iter)?),
                "organization" | "org" => {
                    out.organization = Some(take_value(&arg, &mut iter)?);
                }
                "device" | "deviceid" => out.device = Some(take_value(&arg, &mut iter)?),
                _ => return Err(CliError::Unknown(arg)),
            }
        }

        Ok(out)
    }
}

fn take_value<I>(flag: &str, iter: &mut I) -> Result<String, CliError>
where
    I: Iterator<Item = OsString>,
{
    iter.next()
        .map(|v| v.to_string_lossy().into_owned())
        .ok_or_else(|| CliError::MissingValue(flag.to_owned()))
}

/// Static usage banner printed by `--help`.
pub const USAGE: &str = "\
cmremote-agent — CMRemote managed-endpoint agent (Rust, clean-room)

USAGE:
    cmremote-agent [OPTIONS]

OPTIONS:
    --config <PATH>           Path to ConnectionInfo.json
    --host <URL>              Override the server host
    --organization <ID>       Override the organization id
    --device <ID>             Override the device id
    -h, --help                Print this help and exit
    -v, --version             Print version and exit
";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<CliArgs, CliError> {
        CliArgs::parse(argv.iter().map(|s| OsString::from(*s)))
    }

    #[test]
    fn parses_long_flags() {
        let a = parse(&[
            "cmremote-agent",
            "--host",
            "https://example.com",
            "--organization",
            "org-1",
            "--device",
            "dev-1",
        ])
        .unwrap();
        assert_eq!(a.host.as_deref(), Some("https://example.com"));
        assert_eq!(a.organization.as_deref(), Some("org-1"));
        assert_eq!(a.device.as_deref(), Some("dev-1"));
    }

    #[test]
    fn accepts_short_aliases() {
        let a = parse(&["agent", "--org", "o", "--deviceid", "d"]).unwrap();
        assert_eq!(a.organization.as_deref(), Some("o"));
        assert_eq!(a.device.as_deref(), Some("d"));
    }

    #[test]
    fn help_and_version() {
        assert!(parse(&["agent", "--help"]).unwrap().help);
        assert!(parse(&["agent", "-v"]).unwrap().version);
    }

    #[test]
    fn missing_value_is_an_error() {
        let err = parse(&["agent", "--host"]).unwrap_err();
        assert!(matches!(err, CliError::MissingValue(_)));
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let err = parse(&["agent", "--nope"]).unwrap_err();
        assert!(matches!(err, CliError::Unknown(_)));
    }
}
