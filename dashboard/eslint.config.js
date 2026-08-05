import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'
import { defineConfig, globalIgnores } from 'eslint/config'

export default defineConfig([
  globalIgnores(['dist']),
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    rules: {
      // Honor the `_`-prefix convention for intentionally unused parameters
      // (e.g. stubbed endpoints, test mocks).
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],

      // ── Newly ENFORCED by the ESLint 10 upgrade (2026-08-05) ──────────────
      // eslint-plugin-react-hooks is unchanged at 7.1.1; ESLint 9 simply was
      // not applying these two rules from its recommended set, and ESLint 10
      // is. They surface 33 PRE-EXISTING findings, not regressions:
      //
      //   react-hooks/set-state-in-effect (29) — the codebase's standard
      //     data-loading shape: an effect calls a useCallback that setStates
      //     synchronously (loading=true) before awaiting.
      //   react-hooks/purity (4) — Date.now() read during render to compute
      //     relative times.
      //
      // Both are legitimate criticisms and both need behavioural refactors
      // across ~15 components. Bundling that into a toolchain upgrade would
      // mean shipping a broad React change whose risk has nothing to do with
      // the upgrade, so they are WARNINGS here: visible on every lint run,
      // not blocking. Tracked for a dedicated pass — see the issue linked from
      // the ESLint 10 PR. Do not silence them.
      'react-hooks/set-state-in-effect': 'warn',
      'react-hooks/purity': 'warn',
    },
  },
])
