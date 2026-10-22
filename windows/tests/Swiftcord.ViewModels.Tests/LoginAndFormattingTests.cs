using System.Globalization;
using System.Net;
using Swiftcord.Core;
using Swiftcord.Core.Rest;
using Swiftcord.Core.Tests.Support;
using Swiftcord.ViewModels.Formatting;
using Swiftcord.ViewModels.Login;
using Swiftcord.ViewModels.Messages;

namespace Swiftcord.ViewModels.Tests;

public sealed class LoginViewModelTests
{
    private static (LoginViewModel Login, FakeHttpHandler Http) Create(HttpStatusCode status)
    {
        var http = new FakeHttpHandler((_, _) => status == HttpStatusCode.OK
            ? FakeHttpHandler.Json("""{"id":"1","username":"me"}""")
            : FakeHttpHandler.Json("""{"message":"401: Unauthorized","code":0}""", status));
        return (new LoginViewModel(o => new DiscordRestClient(o, http)), http);
    }

    [Fact]
    public async Task Observed_tokens_are_verified_before_logging_in()
    {
        var (login, http) = Create(HttpStatusCode.OK);
        string? token = null;
        login.LoggedIn += (t, type, user) => token = t;

        Assert.True(await login.OnTokenObservedAsync("abc.def"));
        Assert.Equal("abc.def", token);
        Assert.Equal("abc.def", http.Requests.Single().Request.Headers.GetValues("Authorization").Single());
    }

    [Fact]
    public async Task Rejected_observed_tokens_are_not_retried_and_show_no_error()
    {
        var (login, http) = Create(HttpStatusCode.Unauthorized);
        Assert.False(await login.OnTokenObservedAsync("stale"));
        Assert.False(await login.OnTokenObservedAsync("stale"));
        Assert.Single(http.Requests);
        Assert.Null(login.Error);
    }

    [Fact]
    public async Task Manual_bot_tokens_strip_the_prefix_and_report_errors()
    {
        var (login, http) = Create(HttpStatusCode.Unauthorized);
        login.IsBotToken = true;
        login.ManualToken = "Bot xyz";

        await login.SubmitManualTokenCommand.ExecuteAsync(null);

        Assert.Equal("Bot xyz", http.Requests.Single().Request.Headers.GetValues("Authorization").Single());
        Assert.NotNull(login.Error);
    }
}

public sealed class FormatterTests
{
    private static readonly CultureInfo Us = CultureInfo.GetCultureInfo("en-US");

    [Theory]
    [InlineData(-30, "a few seconds ago")]
    [InlineData(-60 * 5, "5 minutes ago")]
    [InlineData(60 * 60 * 3, "in 3 hours")]
    [InlineData(-86400, "1 day ago")]
    [InlineData(86400 * 60, "in 2 months")]
    [InlineData(-86400 * 800, "2 years ago")]
    public void Relative_times(int seconds, string expected)
    {
        var now = DateTimeOffset.Parse("2024-05-01T12:00:00Z");
        Assert.Equal(expected, Formatters.Relative(now.AddSeconds(seconds), now));
    }

    [Fact]
    public void Message_timestamps_say_today_and_yesterday()
    {
        var now = DateTimeOffset.Now;
        Assert.StartsWith("Today at ", Formatters.MessageTimestamp(now.AddMinutes(-1 * Math.Min(1, now.TimeOfDay.TotalMinutes)), now, Us));
        Assert.StartsWith("Yesterday at ", Formatters.MessageTimestamp(now.AddDays(-1), now, Us));
        Assert.DoesNotContain(" at ", Formatters.MessageTimestamp(now.AddDays(-10), now, Us));
    }

    [Theory]
    [InlineData(new string[0], "")]
    [InlineData(new[] { "A" }, "A is typing…")]
    [InlineData(new[] { "A", "B", "C" }, "A, B, and C are typing…")]
    [InlineData(new[] { "A", "B", "C", "D" }, "Several people are typing…")]
    public void Typing_text(string[] names, string expected) => Assert.Equal(expected, Formatters.TypingText(names));

    [Theory]
    [InlineData(512, "512 B")]
    [InlineData(1536, "1.5 KB")]
    [InlineData(5 * 1024 * 1024, "5 MB")]
    public void File_sizes(long bytes, string expected)
    {
        CultureInfo.CurrentCulture = Us;
        Assert.Equal(expected, Formatters.FileSize(bytes));
    }

    [Theory]
    [InlineData(4000, 3000, 400, 300)]
    [InlineData(100, 50, 100, 50)]
    [InlineData(300, 1200, 75, 300)]
    public void Image_previews_fit_within_bounds(double w, double h, double ew, double eh) =>
        Assert.Equal((ew, eh), AttachmentViewModel.Fit(w, h));
}
