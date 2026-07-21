# UI/UX Design Audit — Networker Dashboard

**Date:** 2026-07-18 · **Method:** Live visual review of https://alethedash.com (Chrome DevTools MCP, admin session, read-only) + code cross-check of `dashboard/src`.
**North star:** `.impeccable.md` — terminal/hacker aesthetic, monospace-first, dark theme, data-dense, zero chrome; purple `#863bff` logo, cyan primary accent, deep navy backgrounds. References: Grafana, Datadog, **Warp**. Brief: modern-terminal (Warp/Linear polish), *not* retro-DOS kitsch.

**Caveats:** Mobile (390 px) live check was not possible — the `resize_page`/`emulate` tools were permission-denied in this session. Mobile findings below are code-level only (Sidebar has a proper `md:hidden` hamburger + overlay at `Sidebar.tsx:173-183`; tables have few `sm:`/`md:` variants, so table overflow at 390 px is likely but unverified). The prod backend also briefly 502'd during review — which usefully exposed the error-state design (see F2, F3).

---

## Per-surface findings

### 1. Login (`/login`)
**Seen:** Centered wordmark "Networker" in **green**, "NETWORK DIAGNOSTICS" sub-line, terminal-prompt inputs (`>` caret), full-width cyan "Continue" button, two-step email→password reveal.

- **F1 — Wordmark is green, brand says purple.** `LoginPage.tsx:159` hardcodes `text-[#4ade80]` (green-400); prompt carets and focus borders are also green (`text-green-600/60`, `focus-within:border-green-500/50`, lines 205-225). The sidebar project name repeats this (`ProjectSwitcher.tsx:96,114` — `text-green-400`). The brand purple `#863bff` appears **nowhere** in the product UI except the avatar disc and the docs command palette (`components/docs/CommandPalette.tsx`). Green reads "generic hacker terminal", not the Networker brand. This is the single clearest "old and odd" tell.
- The two-step reveal (Continue → password appears, button relabels "Sign in") is nice progressive disclosure and works well.
- Density is fine for a login; dark base `#0a0b0f` correct.
- Minor: domain (alethedash.com) vs product name (Networker) — one stale tab still titled "AletheDash". Cosmetic, but pick one brand string everywhere.

### 2. Post-login → Dashboard (`/projects/:id`) — **CRASHES**
**Seen:** Pure black screen. `#root` is empty. Console: `Uncaught TypeError: Cannot read properties of undefined (reading 'filter')`. Reproducible on reload with a healthy backend.

- **F2 — P0. The project home page is dead for every user, and there is no error boundary.** Root cause verified live: `GET /api/projects/:id/agents` now returns a **bare array** (C# control-plane contract drift), but `client.ts:387-388` types it as `{ agents: Agent[] }` and `DashboardPage.tsx:47` does `r => setAgents(r.agents)` → `undefined` → `agents.filter(...)` at `DashboardPage.tsx:61` throws during render, unmounting the entire app. No React error boundary catches it — the user gets a black void straight after login.
  - Fix: normalize in the client (`Array.isArray(r) ? r : r.agents ?? []` — the same defensive pattern already used by `getTestRunAttempts`, `client.ts:998-1001`), and add a top-level error boundary with a styled "something broke — reload / report" panel.

### 3. Runs list (`/runs`)
**Seen:** Tabs (All 7 / Network 6 / Proxy 1 / Runtime 0), status+type filters, dense mono table, cyan run-id links, type badges (network=cyan, proxy=purple), status pills (completed=green, failed=red), `7 / 2` result column, relative timestamps, pagination, floating perf widget bottom-right.

- Overall the strongest surface: data-dense, flat, thin borders, scannable — genuinely Warp/Grafana-class bones.
- **F4 — Result format chaos.** The same pass/fail data renders as `7 / 2` here (`RunsPage.tsx:441-444`), `7 ok / 2 fail` in the mobile cards (`RunsPage.tsx:392-393`), `7/9 · 2 fail` on the Network wizard's recent-runs hero (`NetworkTestPage.tsx:479`), and `7 ok 2 fail` on URL Probe. Four formats for one number violates "Trust through consistency" — pick one (`7/9 · 2 fail` is the most informative) and make it a shared component.
- **F5 — `0/0 · 0 fail` rendered in red.** Runs that never executed show "0 fail" in failure-red. Zero failures should never wear the failure color; the red should key off `status === 'failed'`, not the count row styling.
- Name column truncates at ~30 chars ("Diag: www.microsoft.c…") while the Modes column wraps to 3 lines — column width allocation favors the least-scannable column.
- "Runtime 0" tab shows even when empty — fine, but consider dimming count-0 tabs.
- Type badge uses purple for `proxy` — purple is reserved for the logo per the north star (see F12).

### 4. Run detail (`/runs/:id`) — **DEAD END**
**Seen:** `Runs / Run 4730342a` breadcrumb, then a red banner: **"Failed to load run — ApiError: API error: 404 Not Found"**. During the 502 window it instead dumped the **entire raw nginx 502 HTML page, including `<!-- a padding to disable MSIE... -->` comments**, into the banner.

- **F3 — P0/P1 pair.**
  1. *Contract drift:* `/api/v2/test-runs/:id` returns **200**, but `/api/v2/test-runs/:id/attempts` returns **404** (verified live). `RunDetailPage.tsx:50-60` treats the attempts failure as fatal (`setError(String(e))`) even though the run loaded — so every run detail linked from the Runs list is a dead end.
  2. *Error copy:* `String(e)` produces `ApiError: API error: 404 Not Found` (double "error", class name leaked) and, for gateway errors, raw HTML (`client.ts:123-130` puts the raw body into `message`). No retry button, no "Back to runs" link — a true dead end.
  - Fix: render what loaded, degrade the attempts section ("Attempt data unavailable"); map status codes to human copy ("This run no longer exists" / "Server unavailable — retrying…"); never print a response body verbatim.

### 5. Infrastructure (`/vms`)
**Seen:** KPI band (5 RUNNERS ACTIVE — the "5" in purple; 5 TARGETS; 3 CLOUD ACCOUNTS), "+ Deploy" cyan, "+ runner"/"+ target" ghost buttons, runner groups by cloud/region, targets table, recent-activity strip. Tagline: "Runners do the work; targets are what they probe." (good).

- **F6 — "5 RUNNERS ACTIVE" is false.** The sub-line says `1 idle · 0 busy · 3 stopped · 1 error` — only 1 of 5 is operational. "Active" apparently means "not archived". For an SRE audience this is a trust-breaking label; say "5 runners" or "1 ready / 5".
- **F7 — Status double-speak.** A runner row shows a green `running` pill *and* right-aligned text `idle` (VM power state vs agent state, unlabeled); stopped rows say "stopped" twice. Two vocabularies on one row with no affordance distinguishing them.
- `v—` shown for unknown runner version (and `· v?` in the URL-probe runner picker) — two different unknown-version placeholders; pick one (`v?`).
- Purple KPI number: rogue accent (see F12).
- Casing drift: "+ Deploy" vs "+ runner" / "+ target" / "+ add to us-east-1" (see F13).

### 6. URL Probe (`/probe`)
**Seen:** Prompt-style probe bar (URL: > …, Preset, Runner, cyan ▶ run button), Recent chips, summary line `3 URLs · 0 healthy · 3 failed · 0 stale`, watched-URL rows (red text + red left border), expandable run history.

- **F8 — P1. Raw ANSI escape codes rendered as text.** Expanding a row shows: `[tester] [2m2026-07-14T01:22:24.974248Z [0m [32m INFO [0m [2mnetworker_tester …`. The tester's ANSI-colored log line is stored in `error_message` and rendered verbatim (`DiagnosticsPage.tsx:417-420`). Nothing says "retro-DOS gone wrong" louder than leaked SGR codes. Strip ANSI at the backend (preferred) or with a small strip util at render.
- **F9 — Verdict contradiction.** www.cloudflare.com is red/"failed" here (2 of 9 modes failed) while the Runs page shows the same run as a green `completed`. Same entity, opposite verdicts on adjacent pages. Define one rule (e.g. completed-with-failures = amber "partial") and use it everywhere.
- Zero-counts are colored (`0 healthy` green, `0 stale` amber) — color should carry signal; grey out zeros.
- Row metric `total 51.3s` is unlabeled (total of what? wall-clock of the full run) — rename ("run 51.3s" or move to tooltip).
- Nav label "URL" vs page title "URL Probe" — make the nav say "URL Probe".
- Good: preset durations ("Quick (~3s)"), runner "auto-pick" with explanatory tooltip, keyboard hints (↑↓ ↵ esc).

### 7. New Network Test (`/tests/new`)
**Seen:** Recent-runs rerun list with number-key shortcuts, "OR BUILD A NEW RUN" divider, 3 numbered steps, mode chips grouped in four *colored* categories (NETWORK=green, HTTP=cyan, THROUGHPUT=purple, PAGE-LOAD=yellow), target search, runner auto-pick, sticky footer with `⏎ launch` hints.

- **F10 — Design jargon in user copy:** "Most runs repeat — use the **hero** below for the fastest path." Users don't know what a "hero" is — and it's *above* the builder, not below. (`NetworkTestPage.tsx:364`.)
- **F11 — P1. The floating perf widget overlaps the form.** The `ApiLogPanel` collapsed pill is `fixed bottom-4 right-4 z-50` (`ApiLogPanel.tsx:92`) and sat directly on top of the target-search input's right edge and the footer hints. Selected-chip colors follow the four category hues, so a configured form shows green+cyan chips side by side (see F12).
- Mode naming drift vs. Full Stack wizard: lowercase `tcp / tlsresume / pageload2` chips here vs `TCP / TLS Resume / Page Load (Native) H2` checkboxes there — same concepts, two vocabularies (`pageload2` is opaque; the Full Stack labels are strictly better).
- Target dropdown metadata: `azure · ? · nginx, apache` — a literal `?` for unknown region.
- Good: rerun-first workflow, number-key shortcuts, live "6 modes selected" counter, disabled-Launch summary strip (`0 modes · auto-runner`).

### 8. New Full Stack Benchmark (`/benchmarks/full-stack/new`)
**Seen:** 4-step wizard (Testbeds → Workload → Methodology → Review), quick-add testbed chips, TestbedRow (account search, region, topology, instance type, OS toggle, proxies, runner VM), methodology cards (Quick/Standard/Rigorous), review summary.

- **F14 — P1 trust bug: Review contradicts Methodology.** Step 3 selected **Standard (10 warmup / 50 measured / 5% target error)**; Review shows **"5 warmup / 30 measured / 2% target error"**. Whatever launches, the user was told two different things. (FullStackPage review-summary derivation vs methodology state.)
- **F15 — The perf widget blocks the wizard's Next/Launch buttons.** Repeatedly, clicks aimed at "Next" hit the perf pill and toggled a 760 px-wide log panel open over the footer (`ApiLogPanel.tsx:128`, `fixed bottom-0 right-0 z-50 … max-h-[60vh]`). I had to click Next via DOM to finish the walkthrough. On the Settings page the same widget covered an "Update" button. Fix: give pages bottom padding when the pill is present, drop the pill's z-index below page CTAs, or dock the panel so it *pushes* content instead of covering it.
- Silent disabled "Next": with proxies chosen but no cloud account, Next is just grey — no message says *why*. The account field shows a premature amber "warning" border before the user has touched it, and the amber "At least one proxy is required" shows before interaction too.
- Selected-state color split in one form: OS toggle "Linux" selected = **green**, "Server (headless)" and "nginx" selected = **cyan**. One form, two selection colors.
- Empty account dropdown is not exposed to the a11y tree (custom listbox options invisible to screen readers; only the ↑↓/↵/esc hints are).
- Selected account renders as `● Azure · AZURE / —` — trailing `—` placeholder junk.
- Step 2 (Workload) is excellent — right-aligned annotations ("Connect", "Warm handshake", "Chrome QUIC") are exactly the terminal-instrument density the north star wants. Step 3 methodology cards are the best copy in the product.

### 9. New Application Benchmark (`/benchmarks/application/new`)
**Seen:** Template cards (Linux API Stack, Windows API Stack, Proxy Comparison, API Compute, Validation Run, Low Noise, Custom) with `1 testbed / 6 lang` meta.
- Solid. Copy is appropriately technical ("Golden run: Rust + Python, h2 + h3. Validates measurement correctness."). No Next button visible at step 1 and no hint that clicking a card advances — minor.

### 10. Schedules (`/schedules`)
**Seen:** Search + status filter, empty table: "No scheduled tests yet / Schedules are created as part of the New Run wizard."
- Empty state names a "New Run wizard" that doesn't exist in the nav (nav says Network / Full Stack / Application) and provides **no link** — a dead-end empty state. Add a CTA button to the wizard.
- The two filter controls render even with zero items — noise before first value.

### 11. Settings (`/settings`)
**Seen:** Tabs (General / Members / Cloud / Share Links / Approvals), System Health box, "system versions" list, deployed targets with Update buttons, cloud inventory/accounts.

- **F16 — Raw JSON error shown to user:** System Health renders `{"error":"internal server error"}` verbatim in red.
- **F17 — IPs mangled into labels:** endpoint rows are titled "136" and "34" — `host.split('.')[0]` (`SettingsPage.tsx:230`) applied to IP addresses. Label logic must special-case IPs.
- **Cross-surface contradiction:** "No cloud accounts configured" here, while the Full Stack wizard happily found account "Azure · AZURE". One of them is lying (different endpoints, one failing silently).
- Section-header casing flips to lowercase ("system versions", "deployed targets") while tabs are Title Case.
- Dashboard version `v0.28.36` colored amber for no stated reason (amber = update available? unlabeled semantics).

### 12. Workspaces (`/projects`) & chrome
- Terminology split: page says **Workspaces**, routes/sidebar say **projects**, sidebar placeholder said "Select work…". Pick one term.
- Workspace card is bare (name + slug + role) — no run counts or last activity; low information for the product's data-density value.
- Sidebar: green project name (F1), ASCII glyph icons (◆ ▣ ✓ ▷ ▤ ▦ ▶ ↻) are charming and on-theme; ADMIN group uses ⌘ as the "System" icon — a macOS-specific glyph that means "command", odd on Windows/Linux; footer `? Help / Search` hints good.
- Sidebar user block shows `admin / admin` (name / role duplicated) — looks like a rendering bug even when it isn't.

### 13. Cross-cutting: color system (code-verified)
- **F12 — Accent discipline is the biggest systemic gap.** Live surfaces show green (brand/wordmark/selected chips/success), cyan (primary), purple (KPI number, proxy badge, provisioning status, THROUGHPUT category), yellow (PAGE-LOAD category, stale, version), blue (running/deploying/assigned — `StatusBadge.tsx:8-11` — visibly *not* cyan next to cyan links), orange (**cancelled**, `StatusBadge.tsx:14` — the previously flagged cancelled=orange inconsistency is still present), red, grey. That's ~8 hues with overlapping meanings; the north star allows green/red/yellow/cyan status + purple logo only. Decisions needed: status ramp per the `/critique` decision (severity=v2-cyan-ramp), cancelled→grey, running→cyan, provisioning→grey/cyan, category colors→one accent.
- **Token system is skeletal** (`index.css:4-10`): only 5 background/border vars. No brand tokens — purple `#863bff`/cyan `#47bfff` are not defined anywhere central; `btn-primary` hardcodes an oklch cyan. `PhaseBreakdown.tsx` still bypasses tokens with slate hexes (`#0a0f1a`, `#1e293b`, `#0f172a`, `#334155`, `#080d14`, plus `#444`/`#555` greys — lines 98, 144-151, 240-420), giving benchmark panels a subtly different navy tint than `--bg-surface #0d0e14`. `HorizontalBoxWhiskerChart.tsx:166` same (`#1e293b`).

### 14. Cross-cutting: typography & density
- 100% monospace via `body` font stack (`index.css:16`) — including multi-sentence prose (page taglines, empty states, methodology explainer). For this audience it works and mostly reads "Warp", not "DOS"; the thing that makes it feel dated is not the mono, it's the *green wordmark + leaked ANSI + raw errors*. Consider `Cascadia Code` *before* `ui-monospace` so macOS doesn't fall back to SF Mono while Windows gets Cascadia (today's order makes platforms diverge), and consider a sans for long-form prose only.
- Density is genuinely good — KPI bands, annotated checklists, and the Runs table are reference-quality. Zero chrome respected: flat surfaces, thin borders, no gradients/shadows anywhere. Motion is restrained and `prefers-reduced-motion` is handled (`index.css:68,164`) — modern-terminal polish done right.

### 15. Cross-cutting: copy
- Casing is the main offender (F13): Title-Case buttons ("Create Workspace", "+ Add Testbed", "Launch Now") vs lowercase actions ("remove", "view all →", "+ runner", "rerun") vs UPPER labels (RUNNER ASSIGNMENT, EMAIL). The lowercase-verbs style is a legitimate terminal idiom — but then "+ Add Testbed"/"Update" break it. Pick: **lowercase for inline/secondary actions, Title Case for primary CTAs**, and apply it.
- Tone is otherwise strong: "Runners do the work; targets are what they probe", methodology cards, workload annotations — precise and technical per brand.
- Errors are the weak half of the voice: `ApiError: API error: 404`, raw nginx HTML, raw JSON — the product's terminal confidence collapses exactly when things go wrong (which is when SREs are watching most closely).

---

## Scores (0-100)

| Dimension | Score | Rationale |
|---|---|---|
| Color discipline | 48 | Zero-chrome and dark base are right, but ~8 competing hues, green-as-brand, purple misuse, cancelled=orange, blue-vs-cyan drift, untokenized brand colors |
| Typography | 72 | Confident mono-first, good sizes/weights/alignment; platform-divergent font stack order, mono for long prose, occasional 11px greys near contrast floor |
| Density / layout | 80 | Reference-quality data density, flat and precise; perf-widget occlusion, Runs column allocation, sparse workspace cards |
| Copy quality | 55 | Taglines/annotations excellent; "hero" jargon, casing anarchy, raw ApiError/nginx/JSON leakage, ANSI codes, `v—`/`?`/`—` placeholder junk |
| Flow clarity | 42 | Wizards well-structured with shortcuts, but: home page crashes (P0), run detail dead-ends (P0), Review contradicts Methodology, silent disabled Next, empty states without CTAs, cross-page verdict contradictions |
| Modern vs dated feel | 58 | Bones are Warp-class (density, shortcuts, restraint); green-terminal branding, leaked ANSI/SGR, raw error dumps and status double-speak drag it toward "old and odd" |
| **Overall** | **57** | Excellent skeleton, currently sabotaged by two dead pages, error-state neglect, and an undisciplined accent system |

---

## Prioritized fixes

### P1 — actively hurts the terminal-modern feel or blocks/confuses users
1. **Fix the dashboard crash + add an error boundary** (F2). Normalize `getAgents` response in `dashboard/src/api/client.ts:387` (accept bare array, like `getTestRunAttempts` does at 998-1001); guard `DashboardPage.tsx:61-62`; wrap routes in an error boundary with a styled fallback.
2. **Fix run detail dead end** (F3). Restore/repoint `/api/v2/test-runs/:id/attempts` in the C# control plane; in `RunDetailPage.tsx:50-60` degrade gracefully when attempts fail but the run loads.
3. **Humanize API errors** (F3/F16). In `client.ts:118-131`, map status→copy and never surface raw bodies (cap + detect HTML/JSON); replace the `ApiError: API error:` prefix; add retry/back actions to error banners (`RunDetailPage.tsx:138`, `SystemHealthPanel.tsx`).
4. **Stop the perf widget from covering CTAs** (F11/F15). `ApiLogPanel.tsx:92,128` — lower z-index below page CTAs, add page bottom padding when pill is visible, or dock the expanded panel so it pushes content; never overlap the wizard footer or table action buttons.
5. **Strip ANSI escape codes from displayed logs** (F8). Best at the source (agent/control plane storing `error_message`); belt-and-braces strip in `DiagnosticsPage.tsx:420`.
6. **Make Review match Methodology** (F14). FullStackPage review summary must derive from the selected methodology state (10/50/5% for Standard), not stale defaults.
7. **Re-brand green → purple/cyan** (F1). `LoginPage.tsx:159,205-225`, `ProjectSwitcher.tsx:96,114`: wordmark purple `#863bff`, interactive accents cyan; keep green strictly for success status.

### P2 — inconsistency
8. **One status color system** (F12). `StatusBadge.tsx`: cancelled orange→grey, running/deploying/assigned blue→cyan, provisioning purple→grey; align with the agreed cyan-ramp severity decision; document in `index.css` as tokens.
9. **Tokenize brand + kill hex bypasses.** Add `--accent-cyan`, `--brand-purple`, status tokens to `index.css`; replace slate hexes in `PhaseBreakdown.tsx` (98, 144-151, 240, 274, 318-420) and `HorizontalBoxWhiskerChart.tsx:166` with `--bg-surface`/`--border-default`.
10. **One result format** (F4). Shared `<RunResult ok={} total={} fail={} />`; adopt `7/9 · 2 fail`; stop rendering `0 fail` in red (F5) — key color off run status.
11. **One verdict rule per run** (F9). Completed-with-failures should read identically on Runs, URL Probe, and wizard recent-runs (suggest amber "partial").
12. **One casing convention** (F13). Lowercase for secondary/inline actions, Title Case for primary CTAs; sweep: `remove`, `+ runner`, `+ target`, `view all →`, `+ Add Testbed`, `Update`, `Launch Now`, settings section headers.
13. **One vocabulary for modes.** Replace `pageload2/pageload3` chips in `NetworkTestPage` with the Full Stack wizard's labels (Page Load H2/H3); unify runner state vs VM state wording on Infrastructure rows (F7) and fix "5 RUNNERS ACTIVE" (F6).
14. **Projects vs Workspaces** — pick one term across `/projects` page title, sidebar, and switcher.

### P3 — polish
15. Placeholder junk: `v—` → `v?` everywhere; drop trailing `/ —` in selected account chip; region `?` → `region unknown` tooltip; `Search schedules..` two-dot ellipsis already fixed in code (`SchedulesPage.tsx:226`) — verify deployed.
16. Empty states with CTAs: Schedules → link to the Network wizard; name it consistently ("New Run wizard" doesn't exist).
17. Dim zero-count tabs/counters instead of coloring them (URL Probe summary, Runs "Runtime 0").
18. Settings endpoint labels: don't `split('.')` IPs (`SettingsPage.tsx:230`); show the IP.
19. Rephrase "use the hero below" (`NetworkTestPage.tsx:364`) → "rerun one below, or build a new run".
20. Silent disabled Next in Full Stack step 1 → inline hint "select a cloud account to continue"; don't paint warning-amber on untouched fields.
21. A11y: expose custom dropdown options (account picker) to the a11y tree; the ⌘ "System" icon → platform-neutral glyph; sidebar `admin/admin` name-role dedupe.
22. Font stack order: put `'Cascadia Code'`/`'JetBrains Mono'` before `ui-monospace` (`index.css:16`) for cross-platform consistency; consider a sans face for multi-sentence prose.
23. Mobile: verify 390 px table overflow (couldn't test live — see caveat); add `overflow-x-auto` on `.table-container` if absent.
