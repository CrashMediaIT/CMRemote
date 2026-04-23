using System.Text;

namespace Remotely.Migration.Legacy.Readers;

/// <summary>
/// Per-provider SQL fragments for the keyset-paginated SELECTs that
/// every <see cref="ILegacyRowReader{TLegacy}"/> uses.
///
/// <para>
/// Centralised so the Organization / Device / User readers all get
/// the same identifier-quoting + LIMIT-vs-TOP rules, and so adding a
/// fourth entity (or a fourth provider) is one method here rather
/// than copy-pasting the dialect switch into every reader.
/// </para>
///
/// <para>
/// Pagination is keyset, not OFFSET/LIMIT, because the latter
/// degrades on large tables and can re-read rows when the underlying
/// data shifts mid-import. The cursor parameter name is uniformly
/// <c>@lastId</c>; readers parameterise the column name so a
/// non-<c>ID</c> primary key (e.g. <c>AspNetUsers.Id</c>) still
/// works without bespoke SQL per reader.
/// </para>
/// </summary>
internal static class LegacyKeysetSql
{
    /// <summary>
    /// Returns a per-provider quoted identifier for
    /// <paramref name="raw"/>. SQL Server uses square brackets;
    /// SQLite + PostgreSQL use double quotes (which preserves the
    /// upstream EF Core's mixed-case identifiers on PostgreSQL,
    /// where unquoted identifiers fold to lower-case).
    /// </summary>
    public static string Quote(LegacyDbProvider provider, string raw)
        => provider == LegacyDbProvider.SqlServer ? $"[{raw}]" : $"\"{raw}\"";

    /// <summary>
    /// Builds the per-provider SELECT for one keyset page.
    /// </summary>
    /// <param name="provider">Source provider.</param>
    /// <param name="table">Unquoted table name.</param>
    /// <param name="keyColumn">Unquoted primary-key column to order by + cursor on.</param>
    /// <param name="columns">Unquoted columns to project, in the order the reader expects.</param>
    /// <param name="hasCursor">
    /// True after the first page has run; adds the
    /// <c>WHERE keyCol &gt; @lastId</c> guard.
    /// </param>
    public static string BuildPageQuery(
        LegacyDbProvider provider,
        string table,
        string keyColumn,
        IReadOnlyList<string> columns,
        bool hasCursor)
    {
        var qTable = Quote(provider, table);
        var qKey = Quote(provider, keyColumn);

        var projection = new StringBuilder();
        for (var i = 0; i < columns.Count; i++)
        {
            if (i > 0)
            {
                projection.Append(", ");
            }
            projection.Append(Quote(provider, columns[i]));
        }

        var where = hasCursor ? $"WHERE {qKey} > @lastId " : string.Empty;

        return provider switch
        {
            // SQL Server uses TOP rather than LIMIT.
            LegacyDbProvider.SqlServer =>
                $"SELECT TOP(@batch) {projection} FROM {qTable} {where}ORDER BY {qKey};",

            // SQLite + PostgreSQL share LIMIT.
            _ =>
                $"SELECT {projection} FROM {qTable} {where}ORDER BY {qKey} LIMIT @batch;",
        };
    }
}
