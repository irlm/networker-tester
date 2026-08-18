# LagHound dashboard

React 19 + TypeScript + Vite frontend for the LagHound control plane.

## Development

```bash
npm install
npm run dev
```

The development server proxies `/api` to the configured control plane. Production builds are static assets served by nginx.

## Commands

```bash
npm run lint
npm test
npm run build
npm run check:bundle-size
npm run test:e2e
npm run test:e2e:a11y
npm run test:e2e:responsive
npm run test:e2e:bundle
```

## Structure

- `src/app`: application providers and query policy
- `src/features`: domain-owned API, queries, and feature UI
- `src/pages`: route composition
- `src/components/common`: reusable design-system primitives
- `src/api/http.ts`: shared REST transport
- `src/api/client.ts`: legacy endpoint façade being migrated into features
- `src/stores`: client-only global state

Read [ARCHITECTURE.md](./ARCHITECTURE.md) before adding a page, endpoint, polling loop, dialog, or reusable control. The repository-level [DESIGN.md](../DESIGN.md) is the visual contract.

Playwright boots the production build by default and uses the shared deterministic
runtime in `e2e/support/runtime.ts`. Set `PLAYWRIGHT_BASE_URL` to compare the
established route and journey suite with a deployed environment.
