# Dashboard architecture

The dashboard uses a feature-oriented React architecture. Route pages compose features; features own server operations; shared UI stays domain-free.

## Dependency direction

```text
main / app providers
        ↓
route pages
        ↓
feature modules ──────→ shared UI
        ↓                  ↓
API transport          hooks / utilities
```

Dependencies point downward. `components/common` must never import a page or feature. Feature modules may use shared UI and the HTTP transport. Pages may compose multiple features, but must not create a second transport or duplicate query behavior.

## Folder ownership

- `src/app/`: application-wide providers and configuration. `queryClient.ts` defines the one server-state policy.
- `src/features/<feature>/`: a vertical slice. Keep its transport in `api.ts`, query keys/hooks in `queries.ts`, and feature-only components beside them.
- `src/pages/`: route composition, URL state, permissions, and page layout. Move reusable business behavior into its feature.
- `src/components/common/`: domain-free UI primitives such as `Button`, `Modal`, `ConfirmDialog`, `FormControls`, `DataTable`, `PageShell`, and async states.
- `src/components/<area>/`: reusable UI tied to one product area. It may import that feature, but not route pages.
- `src/api/http.ts`: the only low-level REST transport. It owns auth headers, multipart handling, errors, unauthorized redirects, cancellation, and request instrumentation.
- `src/api/client.ts`: legacy endpoint façade while remaining domains migrate into features. Do not add new feature endpoints here.
- `src/stores/`: durable client state only: session, active workspace, live connection state, preferences, and toasts.
- `src/hooks/`: browser or cross-feature behavior. Server data does not belong in a polling hook.

## State ownership

Use the narrowest owner:

1. Remote/server state: TanStack Query. Query functions accept its `AbortSignal`; mutations invalidate the affected keys.
2. URL state: route params and search params for shareable filters or navigation state.
3. Cross-route client state: Zustand only when the value must survive route changes.
4. Local interaction state: `useState` for open panels, draft fields, and temporary selections.

Do not copy query results into Zustand or mirror them in component state. Polling is a query option (`refetchInterval`), not a page-owned timer. Keep previous list data while filters or cursors change when a blank state would be misleading.

## Reuse rules

- Use `Button` or `buttonClassName` for actions and action links. Status colors describe state; cyan is the interactive voice.
- Use `Input`, `Select`, `Textarea`, and `FormField` for form controls and accessible help/error wiring.
- Use `Modal` for centered dialogs and slide-over drawers. It owns Escape, backdrop dismissal, focus trapping/restoration, and ARIA. Use `ConfirmDialog` for irreversible actions.
- Use `PageShell` for route padding and title/action composition; use `LoadingState` and `ErrorState` for full-page async states.
- Use `DataTable` for normal tabular data. A bespoke table is justified only by virtualization, complex row grouping, or a specialized interactive layout.
- Extract repeated business UI into its feature before adding another page-local copy.
- Keep large optional dependencies behind the smallest truthful render boundary. Recharts is loaded by feature-specific lazy chart components only when chartable data exists; table-only and empty states must not pay that cost.

## Feature API pattern

```ts
// features/widgets/api.ts
export const widgetsApi = {
  list: (projectId: string, signal?: AbortSignal) =>
    request<Widget[]>(`/projects/${projectId}/widgets`, { signal }),
};

// features/widgets/queries.ts
export const widgetKeys = {
  all: ['widgets'] as const,
  list: (projectId: string) => [...widgetKeys.all, 'list', projectId] as const,
};
```

Query keys are factories, start with the feature name, and include every argument that changes the response. A feature exports task-level hooks rather than exposing cache details to pages.

## Testing and changes

- Test transport behavior at `api/http.ts` through public feature APIs.
- Test query hooks for keys, cancellation, invalidation, and polling policy.
- Test shared primitives once for keyboard and accessibility behavior; feature tests should focus on domain outcomes.
- Reuse `e2e/support/runtime.ts` for deterministic browser API/session fixtures. Do not copy route stubs into a new spec.
- Keep representative WCAG AA coverage in `e2e/accessibility.spec.ts` and mobile/tablet/desktop overflow coverage in `e2e/responsive.spec.ts`.
- Put critical URL-to-request behavior in a focused browser spec; `e2e/perf-log.spec.ts` is the reference for URL-backed server filters.
- Protect demand-loaded bundle boundaries in a browser test; `e2e/lazy-charts.spec.ts` verifies both the no-data fast path and the chart-data load path against the production build.
- Before shipping frontend changes run `npm run lint`, `npm test`, `npm run build`, `npm run check:bundle-size`, and the relevant Playwright suite.

`PLAYWRIGHT_BASE_URL=https://… npx playwright test` runs against a deployed
bundle without starting Vite. Use the stable route, app, flow, and journey
suites for production parity; new behavior that is not deployed yet belongs
in the local suite until release.

When touching legacy code, move the endpoint and server-state behavior into its feature in the same change when practical. Avoid broad rewrites that mix unrelated domains.
