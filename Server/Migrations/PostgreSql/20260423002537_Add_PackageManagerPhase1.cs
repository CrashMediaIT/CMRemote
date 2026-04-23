using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.PostgreSql
{
    /// <inheritdoc />
    public partial class Add_PackageManagerPhase1 : Migration
    {
        /// <inheritdoc />
        protected override void Up(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.AddColumn<bool>(
                name: "PackageManagerEnabled",
                table: "Organizations",
                type: "boolean",
                nullable: false,
                defaultValue: false);

            migrationBuilder.CreateTable(
                name: "DeviceInstalledApplicationsSnapshots",
                columns: table => new
                {
                    DeviceId = table.Column<string>(type: "text", nullable: false),
                    FetchedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    ApplicationsJson = table.Column<string>(type: "text", nullable: false)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_DeviceInstalledApplicationsSnapshots", x => x.DeviceId);
                    table.ForeignKey(
                        name: "FK_DeviceInstalledApplicationsSnapshots_Devices_DeviceId",
                        column: x => x.DeviceId,
                        principalTable: "Devices",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Cascade);
                });
        }

        /// <inheritdoc />
        protected override void Down(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.DropTable(
                name: "DeviceInstalledApplicationsSnapshots");

            migrationBuilder.DropColumn(
                name: "PackageManagerEnabled",
                table: "Organizations");
        }
    }
}
