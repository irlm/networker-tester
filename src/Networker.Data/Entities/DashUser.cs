using System;
using System.Collections.Generic;

namespace Networker.Data.Entities;

public partial class DashUser
{
    public Guid UserId { get; set; }

    public string? Email { get; set; }

    public string? PasswordHash { get; set; }

    public string Role { get; set; } = null!;

    public DateTime CreatedAt { get; set; }

    public DateTime? LastLoginAt { get; set; }

    public bool MustChangePassword { get; set; }

    public string? PasswordResetToken { get; set; }

    public DateTime? PasswordResetExpires { get; set; }

    public string Status { get; set; } = null!;

    public string AuthProvider { get; set; } = null!;

    public string? SsoSubjectId { get; set; }

    public string? DisplayName { get; set; }

    public string? AvatarUrl { get; set; }

    public bool SsoOnly { get; set; }

    public bool IsPlatformAdmin { get; set; }
}
