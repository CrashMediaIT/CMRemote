using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.PostgreSql
{
    /// <inheritdoc />
    public partial class Add_AgentUpgradePipeline : Migration
    {
        /// <inheritdoc />
        protected override void Up(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.CreateTable(
                name: "AgentUpgradeStatuses",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    DeviceId = table.Column<string>(type: "character varying(128)", maxLength: 128, nullable: false),
                    OrganizationID = table.Column<string>(type: "character varying(128)", maxLength: 128, nullable: false),
                    FromVersion = table.Column<string>(type: "character varying(64)", maxLength: 64, nullable: true),
                    ToVersion = table.Column<string>(type: "character varying(64)", maxLength: 64, nullable: true),
                    State = table.Column<int>(type: "integer", nullable: false),
                    CreatedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    EligibleAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    LastAttemptAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: true),
                    CompletedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: true),
                    LastAttemptError = table.Column<string>(type: "character varying(2048)", maxLength: 2048, nullable: true),
                    AttemptCount = table.Column<int>(type: "integer", nullable: false)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_AgentUpgradeStatuses", x => x.Id);
                });

            migrationBuilder.CreateIndex(
                name: "IX_AgentUpgradeStatuses_DeviceId",
                table: "AgentUpgradeStatuses",
                column: "DeviceId",
                unique: true);

            migrationBuilder.CreateIndex(
                name: "IX_AgentUpgradeStatuses_OrganizationID",
                table: "AgentUpgradeStatuses",
                column: "OrganizationID");

            migrationBuilder.CreateIndex(
                name: "IX_AgentUpgradeStatuses_State_EligibleAt",
                table: "AgentUpgradeStatuses",
                columns: new[] { "State", "EligibleAt" });
        }

        /// <inheritdoc />
        protected override void Down(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.DropTable(
                name: "AgentUpgradeStatuses");
        }
    }
}
