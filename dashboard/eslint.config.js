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

      // ── Enforced (was 'warn' for one release while the backlog cleared) ──
      // The ESLint 10 upgrade surfaced 33 findings from these two rules; all of
      // them are now fixed, so they are errors again. Keep them that way: the
      // fixes were derive-during-render, lazy state initialisers, and a
      // useAsyncEffect hook that moves genuinely reactive updates off the
      // effect's synchronous body. Reintroducing a synchronous setState in an
      // effect should fail the build, not add a warning nobody reads.
      'react-hooks/set-state-in-effect': 'error',
      'react-hooks/purity': 'error',
    },
  },
])
