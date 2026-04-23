using System;
using Microsoft.EntityFrameworkCore.Migrations;

#nullable disable

namespace Remotely.Server.Migrations.PostgreSql
{
    /// <inheritdoc />
    public partial class Add_PackageManagerPhase2 : Migration
    {
        /// <inheritdoc />
        protected override void Up(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.CreateTable(
                name: "DeploymentBundles",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    OrganizationID = table.Column<string>(type: "text", nullable: false),
                    Name = table.Column<string>(type: "character varying(120)", maxLength: 120, nullable: false),
                    Description = table.Column<string>(type: "character varying(1024)", maxLength: 1024, nullable: true),
                    CreatedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    CreatedByUserId = table.Column<string>(type: "text", nullable: true)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_DeploymentBundles", x => x.Id);
                    table.ForeignKey(
                        name: "FK_DeploymentBundles_Organizations_OrganizationID",
                        column: x => x.OrganizationID,
                        principalTable: "Organizations",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Cascade);
                });

            migrationBuilder.CreateTable(
                name: "Packages",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    OrganizationID = table.Column<string>(type: "text", nullable: false),
                    Name = table.Column<string>(type: "character varying(120)", maxLength: 120, nullable: false),
                    Provider = table.Column<int>(type: "integer", nullable: false),
                    PackageIdentifier = table.Column<string>(type: "character varying(256)", maxLength: 256, nullable: false),
                    Version = table.Column<string>(type: "character varying(64)", maxLength: 64, nullable: true),
                    InstallArguments = table.Column<string>(type: "character varying(1024)", maxLength: 1024, nullable: true),
                    Description = table.Column<string>(type: "character varying(1024)", maxLength: 1024, nullable: true),
                    CreatedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    CreatedByUserId = table.Column<string>(type: "text", nullable: true)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_Packages", x => x.Id);
                    table.ForeignKey(
                        name: "FK_Packages_Organizations_OrganizationID",
                        column: x => x.OrganizationID,
                        principalTable: "Organizations",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Cascade);
                });

            migrationBuilder.CreateTable(
                name: "BundleItems",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    DeploymentBundleId = table.Column<Guid>(type: "uuid", nullable: false),
                    PackageId = table.Column<Guid>(type: "uuid", nullable: false),
                    Order = table.Column<int>(type: "integer", nullable: false),
                    ContinueOnFailure = table.Column<bool>(type: "boolean", nullable: false)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_BundleItems", x => x.Id);
                    table.ForeignKey(
                        name: "FK_BundleItems_DeploymentBundles_DeploymentBundleId",
                        column: x => x.DeploymentBundleId,
                        principalTable: "DeploymentBundles",
                        principalColumn: "Id",
                        onDelete: ReferentialAction.Cascade);
                    table.ForeignKey(
                        name: "FK_BundleItems_Packages_PackageId",
                        column: x => x.PackageId,
                        principalTable: "Packages",
                        principalColumn: "Id",
                        onDelete: ReferentialAction.Restrict);
                });

            migrationBuilder.CreateTable(
                name: "PackageInstallJobs",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    OrganizationID = table.Column<string>(type: "text", nullable: false),
                    PackageId = table.Column<Guid>(type: "uuid", nullable: false),
                    DeploymentBundleId = table.Column<Guid>(type: "uuid", nullable: true),
                    DeviceId = table.Column<string>(type: "character varying(128)", maxLength: 128, nullable: false),
                    Action = table.Column<int>(type: "integer", nullable: false),
                    Status = table.Column<int>(type: "integer", nullable: false),
                    CreatedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: false),
                    StartedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: true),
                    CompletedAt = table.Column<DateTimeOffset>(type: "timestamp with time zone", nullable: true),
                    RequestedByUserId = table.Column<string>(type: "character varying(64)", maxLength: 64, nullable: true)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_PackageInstallJobs", x => x.Id);
                    table.ForeignKey(
                        name: "FK_PackageInstallJobs_DeploymentBundles_DeploymentBundleId",
                        column: x => x.DeploymentBundleId,
                        principalTable: "DeploymentBundles",
                        principalColumn: "Id",
                        onDelete: ReferentialAction.SetNull);
                    table.ForeignKey(
                        name: "FK_PackageInstallJobs_Organizations_OrganizationID",
                        column: x => x.OrganizationID,
                        principalTable: "Organizations",
                        principalColumn: "ID",
                        onDelete: ReferentialAction.Cascade);
                    table.ForeignKey(
                        name: "FK_PackageInstallJobs_Packages_PackageId",
                        column: x => x.PackageId,
                        principalTable: "Packages",
                        principalColumn: "Id",
                        onDelete: ReferentialAction.Restrict);
                });

            migrationBuilder.CreateTable(
                name: "PackageInstallResults",
                columns: table => new
                {
                    Id = table.Column<Guid>(type: "uuid", nullable: false),
                    PackageInstallJobId = table.Column<Guid>(type: "uuid", nullable: false),
                    ExitCode = table.Column<int>(type: "integer", nullable: false),
                    Success = table.Column<bool>(type: "boolean", nullable: false),
                    DurationMs = table.Column<long>(type: "bigint", nullable: false),
                    StdoutTail = table.Column<string>(type: "character varying(16384)", maxLength: 16384, nullable: true),
                    StderrTail = table.Column<string>(type: "character varying(16384)", maxLength: 16384, nullable: true),
                    ErrorMessage = table.Column<string>(type: "character varying(1024)", maxLength: 1024, nullable: true)
                },
                constraints: table =>
                {
                    table.PrimaryKey("PK_PackageInstallResults", x => x.Id);
                    table.ForeignKey(
                        name: "FK_PackageInstallResults_PackageInstallJobs_PackageInstallJobId",
                        column: x => x.PackageInstallJobId,
                        principalTable: "PackageInstallJobs",
                        principalColumn: "Id",
                        onDelete: ReferentialAction.Cascade);
                });

            migrationBuilder.CreateIndex(
                name: "IX_BundleItems_DeploymentBundleId",
                table: "BundleItems",
                column: "DeploymentBundleId");

            migrationBuilder.CreateIndex(
                name: "IX_BundleItems_PackageId",
                table: "BundleItems",
                column: "PackageId");

            migrationBuilder.CreateIndex(
                name: "IX_DeploymentBundles_OrganizationID",
                table: "DeploymentBundles",
                column: "OrganizationID");

            migrationBuilder.CreateIndex(
                name: "IX_PackageInstallJobs_DeploymentBundleId",
                table: "PackageInstallJobs",
                column: "DeploymentBundleId");

            migrationBuilder.CreateIndex(
                name: "IX_PackageInstallJobs_DeviceId",
                table: "PackageInstallJobs",
                column: "DeviceId");

            migrationBuilder.CreateIndex(
                name: "IX_PackageInstallJobs_OrganizationID_Status",
                table: "PackageInstallJobs",
                columns: new[] { "OrganizationID", "Status" });

            migrationBuilder.CreateIndex(
                name: "IX_PackageInstallJobs_PackageId",
                table: "PackageInstallJobs",
                column: "PackageId");

            migrationBuilder.CreateIndex(
                name: "IX_PackageInstallResults_PackageInstallJobId",
                table: "PackageInstallResults",
                column: "PackageInstallJobId",
                unique: true);

            migrationBuilder.CreateIndex(
                name: "IX_Packages_OrganizationID",
                table: "Packages",
                column: "OrganizationID");
        }

        /// <inheritdoc />
        protected override void Down(MigrationBuilder migrationBuilder)
        {
            migrationBuilder.DropTable(
                name: "BundleItems");

            migrationBuilder.DropTable(
                name: "PackageInstallResults");

            migrationBuilder.DropTable(
                name: "PackageInstallJobs");

            migrationBuilder.DropTable(
                name: "DeploymentBundles");

            migrationBuilder.DropTable(
                name: "Packages");
        }
    }
}
