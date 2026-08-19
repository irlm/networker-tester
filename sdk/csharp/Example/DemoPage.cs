namespace LagHound.Example;

internal static class DemoPage
{
    public const string Html = """
        <!doctype html>
        <html lang="en">
        <head>
          <meta charset="utf-8">
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <meta name="color-scheme" content="dark">
          <title>LagHound C# SDK demo</title>
          <style>
            :root {
              --base: #0a0b0f;
              --surface: #0d0e14;
              --raised: #12131a;
              --line: #252631;
              --text: #e5e7eb;
              --muted: #9ca3af;
              --faint: #788294;
              --cyan: #47bfff;
              --green: #4ade80;
            }
            * { box-sizing: border-box; }
            body {
              margin: 0;
              min-height: 100vh;
              background: var(--base);
              color: var(--text);
              font: 14px/1.6 "Cascadia Code", "JetBrains Mono", ui-monospace, Consolas, monospace;
            }
            main { width: min(920px, calc(100% - 32px)); margin: 0 auto; padding: 64px 0; }
            header { max-width: 760px; }
            .status { display: flex; align-items: center; gap: 8px; color: var(--muted); font-size: 12px; }
            .dot { width: 7px; height: 7px; border-radius: 50%; background: var(--green); }
            h1 { margin: 18px 0 14px; max-width: 30ch; font-size: 1.25rem; line-height: 1.3; letter-spacing: -0.02em; }
            .lede { max-width: 66ch; margin: 0; color: var(--muted); }
            .signal { margin: 40px 0 0; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; }
            .signal-head { display: flex; justify-content: space-between; gap: 16px; padding: 12px 16px; background: var(--surface); border-bottom: 1px solid var(--line); font-size: 12px; color: var(--muted); }
            .phases { display: grid; grid-template-columns: 1.35fr .65fr; min-height: 112px; }
            .phase { padding: 20px; display: flex; flex-direction: column; justify-content: space-between; }
            .phase + .phase { border-left: 1px solid var(--line); background: var(--raised); }
            .phase strong { color: var(--cyan); font-weight: 600; }
            .phase span { color: var(--faint); font-size: 12px; }
            .actions { display: flex; flex-wrap: wrap; gap: 10px; margin-top: 24px; }
            button, a.button { border: 1px solid #374151; border-radius: 4px; padding: 9px 14px; background: transparent; color: var(--text); font: inherit; text-decoration: none; cursor: pointer; }
            button:hover, a.button:hover, button:focus-visible, a.button:focus-visible { border-color: var(--cyan); outline: none; }
            button:active, a.button:active { transform: translateY(1px); }
            button:disabled { cursor: wait; opacity: .55; }
            #result { min-height: 24px; margin-top: 12px; color: var(--cyan); font-size: 12px; }
            .details { display: grid; grid-template-columns: 1fr 1fr; gap: 32px; margin-top: 48px; padding-top: 28px; border-top: 1px solid var(--line); }
            h2 { margin: 0 0 10px; font-size: 14px; }
            p, pre { margin: 0; }
            .details p { color: var(--muted); font-size: 12px; }
            code { color: var(--cyan); }
            pre { padding: 14px; overflow-x: auto; background: var(--surface); border: 1px solid var(--line); border-radius: 6px; color: var(--muted); font: inherit; font-size: 12px; }
            footer { margin-top: 48px; color: var(--faint); font-size: 12px; }
            footer a { color: var(--cyan); }
            @media (max-width: 640px) {
              main { padding: 36px 0; }
              .phases, .details { grid-template-columns: 1fr; }
              .phase + .phase { border-left: 0; border-top: 1px solid var(--line); }
            }
          </style>
        </head>
        <body>
          <main>
            <header>
              <div class="status"><span class="dot" aria-hidden="true"></span>LIVE REFERENCE · C# · ASP.NET CORE</div>
              <h1>This app can explain where a request spent its time.</h1>
              <p class="lede">A minimal .NET service with the LagHound endpoint mounted at <code>/laghound</code>. LagHound measures the trip from outside; the SDK reports the application-time slice from inside.</p>
            </header>

            <section class="signal" aria-labelledby="signal-title">
              <div class="signal-head"><span id="signal-title">REQUEST SIGNAL</span><span>endpoint contract v1</span></div>
              <div class="phases">
                <div class="phase"><strong>DNS → TCP → TLS → transfer</strong><span>measured by a LagHound runner</span></div>
                <div class="phase"><strong>application</strong><span>reported by this C# SDK</span></div>
              </div>
            </section>

            <div class="actions">
              <button type="button" id="run-work">Run the 30 ms handler</button>
              <a class="button" href="https://github.com/irlm/networker-tester/tree/main/sdk/csharp/Example">View C# source</a>
            </div>
            <p id="result" aria-live="polite"></p>

            <section class="details">
              <div>
                <h2>Mount it</h2>
                <pre><code>builder.Services.AddLagHound(options =&gt; {
          options.Token = configuration["LAGHOUND_TOKEN"];
        });
        app.UseLagHound();</code></pre>
              </div>
              <div>
                <h2>Probe it</h2>
                <p>Register this app's base URL in LagHound with route <code>/laghound/echo</code>. The diagnostic routes are token-gated, rate-limited, and return a plain 404 without authorization.</p>
              </div>
            </section>

            <footer>LagHound SDK demo · <a href="https://laghound.com">laghound.com</a></footer>
          </main>
          <script>
            const button = document.querySelector('#run-work');
            const result = document.querySelector('#result');
            button.addEventListener('click', async () => {
              button.disabled = true;
              result.textContent = 'running…';
              const started = performance.now();
              try {
                const response = await fetch('/work', { cache: 'no-store' });
                const text = await response.text();
                result.textContent = `${text} · ${Math.round(performance.now() - started)} ms browser round trip`;
              } catch {
                result.textContent = 'Request failed. Try again.';
              } finally {
                button.disabled = false;
              }
            });
          </script>
        </body>
        </html>
        """;
}
