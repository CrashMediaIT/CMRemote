# M3-S1 — `IUserDirectoryService`

## Scope

`IUserDirectoryService` owns the user-directory surface currently exposed by
`IDataService`. This slice moves only user lookup, user creation/deletion,
admin-flag mutation, user options, and display-name writes. Organization,
invite, API-token, and temp-password flows remain in later slices.

| Legacy `IDataService` method | New method |
|---|---|
| `CreateUser(string, bool, string)` | `IUserDirectoryService.CreateUser(...)` |
| `DeleteUser(string, string)` | `IUserDirectoryService.DeleteUser(...)` |
| `GetUserById(string)` | `IUserDirectoryService.GetUserById(...)` |
| `GetUserByName(string, Action<IQueryable<RemotelyUser>>?)` | `IUserDirectoryService.GetUserByName(...)` |
| `GetAllUsersInOrganization(string)` | `IUserDirectoryService.GetAllUsersInOrganization(...)` |
| `GetAllUsersForServer()` | `IUserDirectoryService.GetAllUsersForServer()` |
| `ChangeUserIsAdmin(string, string, bool)` | `IUserDirectoryService.ChangeUserIsAdmin(...)` |
| `SetIsServerAdmin(string, bool, string)` | `IUserDirectoryService.SetIsServerAdmin(...)` |
| `DoesUserExist(string)` | `IUserDirectoryService.DoesUserExist(...)` |
| `GetUserOptions(string)` | `IUserDirectoryService.GetUserOptions(...)` |
| `UpdateUserOptions(string, RemotelyUserOptions)` | `IUserDirectoryService.UpdateUserOptions(...)` |
| `SetDisplayName(RemotelyUser, string)` | `IUserDirectoryService.SetDisplayName(...)` |

## Dependencies

- `IAppDbFactory` for per-call `AppDb` creation.
- `ILogger<UserDirectoryService>` for create/delete failure logging.

No schema, transaction, DTO, authorization-policy, or wire-protocol changes
are part of this slice.

## Invariants

- User-name comparisons that accepted mixed case before remain
  trim/lowercase comparisons.
- `CreateUser` lowercases and trims `UserName` / `Email`, creates default
  `RemotelyUserOptions`, enables lockout, and fails if the organization does
  not exist.
- `DeleteUser` is organization-scoped: a user in another organization is not
  deleted and returns `User not found.`
- `GetAllUsersInOrganization` returns an empty array for blank or unknown org
  IDs.
- `ChangeUserIsAdmin` only updates a user in the supplied organization.
- `SetIsServerAdmin` only succeeds when the caller is already a server admin,
  the target exists, and caller != target.
- `GetUserOptions` returns stored options when present, otherwise a fresh
  default options object.
- `UpdateUserOptions` fails if the user does not exist.
- `SetDisplayName` updates the persisted `UserOptions.DisplayName` for the
  supplied user.

## Caller migration

The following callers are re-pointed from `IDataService` to
`IUserDirectoryService` for the methods listed above:

- `Server/Auth/*RequirementHandler.cs`
- `Server/Auth/ApiAuthorizationFilter.cs`
- `Server/Auth/ExpiringTokenFilter.cs`
- `Server/Services/AuthService.cs`
- `Server/API/DevicesController.cs`
- `Server/API/LoginController.cs`
- `Server/API/OrganizationManagementController.cs`
- `Server/API/RemoteControlController.cs`
- `Server/API/ScriptingController.cs`
- `Server/Components/App.razor`
- `Server/Components/Pages/GetSupport.razor`
- `Server/Components/Pages/ManageOrganization.razor.cs`
- `Server/Components/Pages/ServerConfig.razor.cs`
- `Server/Components/Pages/UserOptions.razor`
- `Server/Pages/Viewer.cshtml.cs`
- `Tests/Server.Tests/TestData.cs`

`IDataService` remains for non-S1 surfaces until later slices, but the S1
methods are removed from the interface and from `DataService` as public
methods in this PR.

## Out of scope

- Invitations and `JoinViaInvitation` (M3-S8).
- API tokens and `ValidateApiKey` (M3-S8).
- Organization lookup/mutation (M3-S2).
- Device permissions and device query methods (M3-S3).
