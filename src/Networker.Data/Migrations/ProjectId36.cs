using System.Security.Cryptography;

namespace Networker.Data.Migrations;

/// <summary>
/// Port of <c>crates/networker-dashboard/src/project_id.rs</c> — the
/// 14-character base36 project identifier with double Damm check digits.
///
/// Layout: <c>ZZ TTTTTT R SSS KK</c>
/// <list type="bullet">
///   <item><c>ZZ</c> — zone code (2 base36 chars)</item>
///   <item><c>TTTTTT</c> — seconds since 2026-01-01T00:00:00Z (6 base36 chars)</item>
///   <item><c>R</c> — 1 random base36 char (collision avoidance)</item>
///   <item><c>SSS</c> — server id (3 base36 chars)</item>
///   <item><c>KK</c> — 2 Damm base36 check digits over the preceding 12 chars</item>
/// </list>
///
/// Needed by the V025 migration (UUID → base36 project ids) so a fresh
/// database migrated by C# produces ids indistinguishable from ones the Rust
/// runner would have produced. The Damm tables are the same affine Latin
/// squares: table1[r][c] = (3r + c) mod 36, table2[r][c] = (5r + c) mod 36,
/// double-pass (pass 2 runs over raw + c1).
/// </summary>
public static class ProjectId36
{
    /// <summary>Seconds from Unix epoch to the project epoch (2026-01-01T00:00:00Z).</summary>
    private const long ProjectEpoch = 1_767_225_600;

    private const string Alphabet = "0123456789abcdefghijklmnopqrstuvwxyz";

    /// <summary>Encode <paramref name="val"/> as base36, zero-padded to <paramref name="width"/> chars.</summary>
    internal static string EncodeBase36(ulong val, int width)
    {
        var digits = new char[width];
        var v = val;
        for (var i = width - 1; i >= 0; i--)
        {
            digits[i] = Alphabet[(int)(v % 36)];
            v /= 36;
        }
        if (v != 0)
        {
            throw new ArgumentOutOfRangeException(nameof(val), $"value {val} overflows {width} base36 digits");
        }
        return new string(digits);
    }

    private static int CharToDigit(char c) => c switch
    {
        >= '0' and <= '9' => c - '0',
        >= 'a' and <= 'z' => c - 'a' + 10,
        _ => -1,
    };

    /// <summary>One Damm pass: table[r][c] = (m*r + c) mod 36. Returns -1 on invalid input.</summary>
    private static int DammPass(ReadOnlySpan<char> s, int m, int start)
    {
        var interim = start;
        foreach (var c in s)
        {
            var d = CharToDigit(c);
            if (d < 0)
            {
                return -1;
            }
            interim = (m * interim + d) % 36;
        }
        return interim;
    }

    /// <summary>Check digit k such that table[interim][k] == 0, i.e. (m*interim + k) mod 36 == 0.</summary>
    private static int CheckDigit(int interim, int m) => (36 - (m * interim) % 36) % 36;

    /// <summary>
    /// Compute the 2 Damm check characters for a 12-char base36 <paramref name="raw"/>:
    /// pass 1 (table1, m=3) over raw → c1; pass 2 (table2, m=5) over raw + c1 → c2.
    /// </summary>
    public static string DammBase36Double(string raw)
    {
        var interim1 = DammPass(raw, 3, 0);
        if (interim1 < 0)
        {
            throw new ArgumentException($"raw must be lowercase base36: {raw}", nameof(raw));
        }
        var c1 = CheckDigit(interim1, 3);

        var interim2 = DammPass(raw + Alphabet[c1], 5, 0);
        var c2 = CheckDigit(interim2, 5);

        return $"{Alphabet[c1]}{Alphabet[c2]}";
    }

    /// <summary>Verify the 2-char <paramref name="check"/> against the 12-char <paramref name="raw"/>.</summary>
    public static bool VerifyDammBase36Double(string raw, string check)
    {
        if (check.Length != 2)
        {
            return false;
        }
        var interim1 = DammPass(raw, 3, 0);
        var c1 = CharToDigit(check[0]);
        if (interim1 < 0 || c1 < 0 || (3 * interim1 + c1) % 36 != 0)
        {
            return false;
        }
        var interim2 = DammPass(raw + check[0], 5, 0);
        var c2 = CharToDigit(check[1]);
        return interim2 >= 0 && c2 >= 0 && (5 * interim2 + c2) % 36 == 0;
    }

    /// <summary>
    /// Generate a project id with an explicit Unix timestamp (seconds) — the
    /// migration path: <paramref name="unixSecs"/> is the original record
    /// creation time, exactly like <c>ProjectId::generate_deterministic</c>.
    /// The R char is random; everything else is deterministic.
    /// </summary>
    public static string GenerateDeterministic(string zone, string serverId, long unixSecs)
    {
        if (zone.Length != 2 || DammPass(zone, 3, 0) < 0)
        {
            throw new ArgumentException("zone must be 2 lowercase base36 chars", nameof(zone));
        }
        if (serverId.Length != 3 || DammPass(serverId, 3, 0) < 0)
        {
            throw new ArgumentException("server_id must be 3 lowercase base36 chars", nameof(serverId));
        }

        var elapsed = (ulong)Math.Max(0, unixSecs - ProjectEpoch);
        var ts = EncodeBase36(elapsed, 6);
        var r = Alphabet[RandomNumberGenerator.GetInt32(36)];

        var raw = $"{zone}{ts}{r}{serverId}";
        return raw + DammBase36Double(raw);
    }

    /// <summary>Generate a project id for the current instant.</summary>
    public static string Generate(string zone, string serverId) =>
        GenerateDeterministic(zone, serverId, DateTimeOffset.UtcNow.ToUnixTimeSeconds());

    /// <summary>Validate: exactly 14 lowercase base36 chars with correct double Damm check.</summary>
    public static bool Validate(string s)
    {
        if (s.Length != 14)
        {
            return false;
        }
        foreach (var c in s)
        {
            if (CharToDigit(c) < 0)
            {
                return false;
            }
        }
        return VerifyDammBase36Double(s[..12], s[12..]);
    }
}
