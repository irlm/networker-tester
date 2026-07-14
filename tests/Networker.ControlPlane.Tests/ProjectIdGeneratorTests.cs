using Networker.ControlPlane.Endpoints;

namespace Networker.ControlPlane.Tests;

/// <summary>
/// The C# port of the Rust <c>project_id.rs</c> generator: 14-char base36 ids
/// with double Damm check digits, plus the <c>slugify</c> helper. Vector tests
/// pin the exact Rust table math (m=3 / m=5 affine Latin squares in Z_36).
/// </summary>
public class ProjectIdGeneratorTests
{
    [Fact]
    public void Generated_id_is_14_lowercase_base36_chars_and_validates()
    {
        var id = ProjectIdGenerator.Generate("us", "a20");

        Assert.Equal(14, id.Length);
        Assert.All(id, c => Assert.True(c is (>= '0' and <= '9') or (>= 'a' and <= 'z')));
        Assert.True(ProjectIdGenerator.Validate(id));
        Assert.StartsWith("us", id);
        Assert.Equal("a20", id.Substring(9, 3));
    }

    [Fact]
    public void Deterministic_generation_is_stable()
    {
        var a = ProjectIdGenerator.GenerateDeterministic("us", "a20", 1_800_000_000, 7);
        var b = ProjectIdGenerator.GenerateDeterministic("us", "a20", 1_800_000_000, 7);
        Assert.Equal(a, b);
        Assert.True(ProjectIdGenerator.Validate(a));
    }

    [Fact]
    public void Single_char_substitution_is_detected()
    {
        var id = ProjectIdGenerator.GenerateDeterministic("us", "a20", 1_800_000_000, 7);
        for (var i = 0; i < 12; i++)
        {
            var mutated = id.ToCharArray();
            mutated[i] = mutated[i] == 'z' ? '0' : (char)(mutated[i] + 1);
            var candidate = new string(mutated);
            if (!candidate.All(c => c is (>= '0' and <= '9') or (>= 'a' and <= 'z')))
            {
                continue;
            }
            Assert.False(ProjectIdGenerator.Validate(candidate),
                $"substitution at index {i} should invalidate the id");
        }
    }

    [Fact]
    public void Adjacent_transpositions_are_detected()
    {
        var id = ProjectIdGenerator.GenerateDeterministic("us", "a20", 1_812_345_678, 19);
        for (var i = 0; i < 11; i++)
        {
            var a = Digit(id[i]);
            var b = Digit(id[i + 1]);
            if (a == b || (a - b) % 18 == 0)
            {
                // Equal chars are a no-op; pairs congruent mod 18 are the
                // documented residual blind spot of the affine double-Damm
                // tables (see project_id.rs), so exclude them here too.
                continue;
            }
            var mutated = id.ToCharArray();
            (mutated[i], mutated[i + 1]) = (mutated[i + 1], mutated[i]);
            Assert.False(ProjectIdGenerator.Validate(new string(mutated)),
                $"transposition at index {i} should invalidate the id");
        }
    }

    private static int Digit(char c) => c <= '9' ? c - '0' : c - 'a' + 10;

    [Theory]
    [InlineData(null)]
    [InlineData("")]
    [InlineData("short")]
    [InlineData("USABCDEF1A2000")]          // uppercase → invalid
    [InlineData("uu1234567a20zz")]          // wrong check digits
    [InlineData("proj-1234567890")]         // wrong alphabet + length
    public void Malformed_ids_fail_validation(string? candidate)
    {
        Assert.False(ProjectIdGenerator.Validate(candidate));
    }

    [Fact]
    public void Check_digits_verify_and_zero_out_per_the_damm_tables()
    {
        // interim after "0" is 0 for both tables, so check digits of "0"*12 are "00".
        var raw = new string('0', 12);
        Assert.Equal("00", ProjectIdGenerator.DammBase36Double(raw));
        Assert.True(ProjectIdGenerator.VerifyDammBase36Double(raw, "00"));
        Assert.False(ProjectIdGenerator.VerifyDammBase36Double(raw, "01"));
        Assert.False(ProjectIdGenerator.VerifyDammBase36Double(raw, "0"));
    }

    [Theory]
    [InlineData(0ul, 6, "000000")]
    [InlineData(35ul, 2, "0z")]
    [InlineData(36ul, 2, "10")]
    [InlineData(46_655ul, 3, "zzz")]
    public void Base36_encoding_matches_rust(ulong value, int width, string expected)
    {
        Assert.Equal(expected, ProjectIdGenerator.EncodeBase36(value, width));
    }

    // ── slugify — same vectors as the Rust db::projects tests ──────────────

    [Theory]
    [InlineData("My Project", "my-project")]
    [InlineData("Test & Demo (v2)", "test-demo-v2")]
    [InlineData("a   b", "a-b")]
    [InlineData("hello-world", "hello-world")]
    [InlineData("", "")]
    public void Slugify_matches_rust(string name, string expected)
    {
        Assert.Equal(expected, ProjectSlug.Slugify(name));
    }
}
