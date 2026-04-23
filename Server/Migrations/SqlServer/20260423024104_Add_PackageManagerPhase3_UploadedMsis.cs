using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.SqlServer
{
    /// <inheritdoc />
    public partial class Add_PackageManagerPhase3_UploadedMsis : Migration
    {
        /// <inheritdoc />
        protected override void Up(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.CreateTable(
                name: "UploadedMsis",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uniqueidentifier", nullable: false),
                    OrganizationID = table.Column<string>(type: "nvarchar(450)", nullable: false),
                    SharedFileId = table.Column<string>(type: "nvarchar(64)", maxLength: 64, nullable: false),
                    Name = table.Column<string>(type: "nvarchar(120)", maxLength: 120, nullable: false),
                    FileName = table.Column<string>(type: "nvarchar(255)", maxLength: 255, nullable: false),
                    SizeBytes = table.Column<long>(type: "bigint", nullable: false),
                    Sha256 = table.Column<string>(type: "nvarchar(64)", maxLength: 64, nullable: false),
                    Description = table.Column<string>(type: "nvarchar(1024)", maxLength: 1024, nullable: true),
                    UploadedAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: false),
                    UploadedByUserId = table.Column<string>(type: "nvarchar(max)", nullable: true),
                    IsTombstoned = table.Column<bool>(type: "bit", nullable: false),
                    TombstonedAt = table.Column<DateTimeOffset>(type: "datetimeoffset", nullable: true)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_UploadedMsis", x => x.Id);
                    table.ForeignKey(
                        name: "FK_UploadedMsis_Organizations_OrganizationID",
                        column: x => x.OrganizationID,
                        principalTable: "Organizations",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Cascade);
                    table.ForeignKey(
                        name: "FK_UploadedMsis_SharedFiles_SharedFileId",
                        column: x => x.SharedFileId,
                        principalTable: "SharedFiles",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Restrict);
                });

            migrationBuilder.CreateIndex(
                name: "IX_UploadedMsis_OrganizationID_IsTombstoned",
                table: "UploadedMsis",
                columns: new[] { "OrganizationID", "IsTombstoned" });

            migrationBuilder.CreateIndex(
                name: "IX_UploadedMsis_Sha256",
                table: "UploadedMsis",
                column: "Sha256");

            migrationBuilder.CreateIndex(
                name: "IX_UploadedMsis_SharedFileId",
                table: "UploadedMsis",
                column: "SharedFileId");
        }

        /// <inheritdoc />
        protected override void Down(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.DropTable(
                name: "UploadedMsis");
        }
    }
}
