// Source: CMRemote, clean-room implementation

using Remotely.Shared.Entities;
using Remotely.Shared.Models;

namespace Remotely.Server.Services.UserDirectory;

public interface IUserDirectoryService
{
    Task ChangeUserIsAdmin(string organizationId, string targetUserId, bool isAdmin);

    Task<Result> CreateUser(string userEmail, bool isAdmin, string organizationId);

    Task<Result> DeleteUser(string orgId, string targetUserId);

    bool DoesUserExist(string userName);

    RemotelyUser[] GetAllUsersForServer();

    Task<RemotelyUser[]> GetAllUsersInOrganization(string orgId);

    Task<Result<RemotelyUser>> GetUserById(string userId);

    Task<Result<RemotelyUser>> GetUserByName(
        string userName,
        Action<IQueryable<RemotelyUser>>? queryBuilder = null);

    Task<Result<RemotelyUserOptions>> GetUserOptions(string userName);

    Task SetDisplayName(RemotelyUser user, string displayName);

    Task SetIsServerAdmin(string targetUserId, bool isServerAdmin, string callerUserId);

    Task<Result> UpdateUserOptions(string userName, RemotelyUserOptions options);
}
