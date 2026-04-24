using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.Sqlite
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
                    Id = table.Column<Guid>(type: "TEXT", nullable: false),
                    DeviceId = table.Column<string>(type: "TEXT", maxLength: 128, nullable: false),
                    OrganizationID = table.Column<string>(type: "TEXT", maxLength: 128, nullable: false),
                    FromVersion = table.Column<string>(type: "TEXT", maxLength: 64, nullable: true),
                    ToVersion = table.Column<string>(type: "TEXT", maxLength: 64, nullable: true),
                    State = table.Column<int>(type: "INTEGER", nullable: false),
                    CreatedAt = table.Column<string>(type: "TEXT", nullable: false),
                    EligibleAt = table.Column<string>(type: "TEXT", nullable: false),
                    LastAttemptAt = table.Column<string>(type: "TEXT", nullable: true),
                    CompletedAt = table.Column<string>(type: "TEXT", nullable: true),
                    LastAttemptError = table.Column<string>(type: "TEXT", maxLength: 2048, nullable: true),
                    AttemptCount = table.Column<int>(type: "INTEGER", nullable: false)
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
