using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.SqlServer
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
                    Id = table.Column<Guid>(type: "uniqueidentifier", nullable: false),
                    DeviceId = table.Column<string>(type: "nvarchar(128)", maxLength: 128, nullable: false),
                    OrganizationID = table.Column<string>(type: "nvarchar(128)", maxLength: 128, nullable: false),
                    FromVersion = table.Column<string>(type: "nvarchar(64)", maxLength: 64, nullable: true),
                    ToVersion = table.Column<string>(type: "nvarchar(64)", maxLength: 64, nullable: true),
                    State = table.Column<int>(type: "int", nullable: false),
                    CreatedAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: false),
                    EligibleAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: false),
                    LastAttemptAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: true),
                    CompletedAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: true),
                    LastAttemptError = table.Column<string>(type: "nvarchar(2048)", maxLength: 2048, nullable: true),
                    AttemptCount = table.Column<int>(type: "int", nullable: false)
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
