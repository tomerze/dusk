import javascript from '@eslint/js'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import globals from 'globals'
import typescript from 'typescript-eslint'

export default typescript.config(
  { ignores: ['dist', 'dist-mock', 'node_modules'] },
  {
    files: ['**/*.{ts,tsx}'],
    extends: [javascript.configs.recommended, ...typescript.configs.recommended],
    languageOptions: {
      ecmaVersion: 2023,
      globals: globals.browser,
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-refresh/only-export-components': ['error', { allowConstantExport: true }],
      'id-length': ['error', { min: 2, properties: 'never' }],
      '@typescript-eslint/no-unused-vars': [
        'error',
        { ignoreRestSiblings: true, args: 'after-used' },
      ],
    },
  },
  {
    files: ['scripts/**/*.mjs', 'vite.config.ts', 'eslint.config.js'],
    languageOptions: { globals: globals.node },
  },
)
