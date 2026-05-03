# CMRemote setup wizard

The first-boot setup wizard is the supported entry point for a new CMRemote v2 deployment and for an in-place cut-over from the legacy Docker image. Until setup is completed, browser requests are redirected to `/setup`; non-browser write requests receive `503 Retry-After: 30` so partially upgraded clients do not silently mutate state.

## When the wizard appears

The wizard appears when the `CMRemote.Setup.Completed` marker is absent from `KeyValueRecords`. Existing deployments that already contain an organization, user, or device are auto-marked complete during startup so an upgrade cannot unexpectedly hijack a live instance into setup.

## Steps

1. **Welcome / preflight**
   - Verifies that the server can write the data directory that will hold `appsettings.Production.json`.
   - Warns if no HTTPS endpoint is configured. HTTP is allowed for development or deployments behind a TLS-terminating reverse proxy.
   - Displays the bound URLs so the operator can confirm the expected address.
   - Blocking failures must be fixed before continuing; warnings do not block setup.

2. **Database connection**
   - Accepts a PostgreSQL connection string and runs a live `SELECT 1` check.
   - Distinguishes invalid connection strings from network or authentication failures.
   - Writes `ConnectionStrings:PostgreSQL` and `ApplicationOptions:DbProvider=PostgreSql` to `appsettings.Production.json` using an atomic temp-file rename.
   - Applies owner-only file mode on Unix (`0600`) and reloads configuration in-process when supported.

3. **Import existing database**
   - Optional for greenfield installs; required when replacing a legacy Docker image without losing data.
   - Provides **Detect**, **Dry-run import**, and **Run import** actions.
   - Uses the same migration runner and converters as the `cmremote-migrate` CLI, so UI and headless imports share one code path.
   - Writes `migration-report.json` next to the wizard settings file for post-run review.

4. **Admin bootstrap**
   - Skipped automatically when an imported database already contains users or organizations.
   - Otherwise creates the first organization and the first server administrator through ASP.NET Identity, preserving the normal password hashing and security-stamp flow.
   - If user creation fails, the organization row is rolled back so the operator can retry cleanly.

5. **Done**
   - Writes the setup-completed marker and advances progress to `Done`.
   - Links the operator to `/Account/Login?returnUrl=%2F`.
   - The wizard cannot be re-run after this point without operator intervention in the database.

## Operational notes

- Keep a backup of the legacy database and CMRemote data volume before starting an import.
- Use **Dry-run import** first. Treat fatal errors or failed rows as blockers until investigated.
- Preserve `migration-report.json` with deployment records; it captures detected schema version, row counts, skipped rows, failures, and fatal errors.
- Do not share screenshots or logs that include connection strings, tokens, or user data.
