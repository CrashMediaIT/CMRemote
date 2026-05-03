# Legacy-to-v2 migration guide

CMRemote v2 supports a non-destructive cut-over from the legacy Docker image to the clean-room server and Rust-agent migration track. The migration path preserves organization, device, and ASP.NET Identity user identities so existing agents and users can reconnect under their existing records.

## Supported paths

- **Setup wizard**: use `/setup` during first boot for interactive migrations.
- **CLI**: use `cmremote migrate --from <sourceConnectionString> --to <targetConnectionString> [--dry-run] [--batch-size N]` for scripted or headless migrations.

Both paths compose the same `MigrationRunner`, readers, converters, and PostgreSQL writers.

## What is migrated in the current migration set

| Entity | Preserved data | Notes |
|---|---|---|
| Organizations | ID, display name, default-organization flag | Names are trimmed and truncated to the v2 25-character cap. |
| Devices | ID, organization ID, core inventory fields, server verification token, last-online timestamp | Devices start offline after migration; the Rust or legacy agent reasserts live state on next check-in. Complex drive and MAC-address inventory is repopulated by the next heartbeat. |
| Users | ASP.NET Identity IDs, user names, normalized names, email fields, password hash, security stamp, concurrency stamp, phone fields, 2FA flag, lockout fields, admin flags, organization ID | Rows without an organization are skipped; rows missing required Identity keys fail loudly. |

The PostgreSQL writers use idempotent upserts so a resumed run can update rows under the same primary keys.

## Recommended procedure

1. Stop scheduled maintenance that may alter the legacy database during the migration window.
2. Back up the legacy database and the CMRemote data volume.
3. Prepare the target PostgreSQL database and credentials.
4. Run schema detection from the setup wizard or CLI.
5. Run a dry run and review the report:
   - `FatalErrors` must be empty.
   - `RowsFailed` should be zero before proceeding.
   - `RowsSkipped` should be understood and documented.
6. Run the real import.
7. Preserve the generated `migration-report.json` or CLI output.
8. Complete the setup wizard, sign in, and verify organizations, users, and devices.
9. Allow agents to check in so online state, drive inventory, and MAC-address inventory refresh from live telemetry.

## CLI exit codes

| Code | Meaning |
|---:|---|
| 0 | Clean run: no fatal errors and no per-row failures. |
| 1 | The run completed but at least one row failed conversion or write. |
| 2 | Fatal error, such as schema detection failure or cancellation. |
| 64 | Usage error, such as a missing `--from` or `--to` argument. |

## Report artifacts

The report records the detected legacy schema version, dry-run state, start and completion timestamps, per-entity rows read / converted / skipped / failed / written, per-entity errors, and fatal errors. The wizard writes this as `migration-report.json` next to `appsettings.Production.json`; the CLI prints the same summary to standard output.

## Security notes

- Treat source and target connection strings as secrets.
- Store backups and migration reports according to your organization's data-retention policy.
- Avoid sharing raw migration reports outside trusted operator channels because row-level errors may include entity identifiers.
