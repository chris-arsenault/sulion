import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactPlugin from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import reactPerf from "eslint-plugin-react-perf";
import jsxA11y from "eslint-plugin-jsx-a11y";
import sonarjs from "eslint-plugin-sonarjs";
import prettier from "eslint-config-prettier";
import globals from "globals";

import localRules from "./eslint-rules/index.js";

export default [
  {
    ignores: ["dist", "build", "coverage", "node_modules", "**/*.min.js"],
  },

  js.configs.recommended,
  ...tseslint.configs.recommended,

  {
    files: ["**/*.{ts,tsx,js,jsx}"],
    plugins: {
      react: reactPlugin,
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
      "react-perf": reactPerf,
      "jsx-a11y": jsxA11y,
      sonarjs,
      local: localRules,
    },
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: { ...globals.browser, ...globals.node },
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    settings: { react: { version: "detect" } },
    rules: {
      // React
      ...reactPlugin.configs.recommended.rules,
      ...reactHooks.configs.recommended.rules,
      "react/react-in-jsx-scope": "off",
      "react/prop-types": "off",
      "react/no-unknown-property": "warn",
      // English apostrophes / quotes in JSX text — React handles them
      // safely and the suggested escapes hurt readability.
      "react/no-unescaped-entities": "off",

      // a11y — start at warn; promote once backlog clears
      ...jsxA11y.configs.recommended.rules,
      "jsx-a11y/no-autofocus": "off",
      "jsx-a11y/click-events-have-key-events": "warn",
      "jsx-a11y/no-static-element-interactions": "warn",
      "jsx-a11y/no-noninteractive-element-interactions": "warn",

      // sonarjs — a couple of high-signal rules; full recommended set
      // is too noisy for an in-flight codebase.
      "sonarjs/no-identical-functions": "warn",
      "sonarjs/no-duplicate-string": "off",
      "sonarjs/cognitive-complexity": ["warn", 25],

      // react-perf — warn so expensive new patterns get flagged but
      // we don't have to chase every literal object.
      "react-perf/jsx-no-new-object-as-prop": "off",
      "react-perf/jsx-no-new-array-as-prop": "off",
      "react-perf/jsx-no-new-function-as-prop": "off",

      // HMR health
      "react-refresh/only-export-components": [
        "warn",
        { allowConstantExport: true },
      ],

      // typescript-eslint
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
      "@typescript-eslint/no-explicit-any": "warn",

      // Local custom rules — load-bearing ones are errors.
      "local/max-jsx-props": ["warn", { max: 14 }],
      "local/no-inline-styles": "warn",
      "local/no-direct-fetch": "error",
      "local/no-non-vitest-testing": "error",
      "local/no-js-file-extension": "error",
      "local/no-raw-undefined-union": "off",
    },
  },

  {
    // Test files: relax rules that don't make sense in tests.
    files: ["**/*.test.{ts,tsx}", "src/test-setup.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
    },
    rules: {
      "react-refresh/only-export-components": "off",
      "@typescript-eslint/no-explicit-any": "off",
      "sonarjs/no-identical-functions": "off",
    },
  },

  {
    // Config files + the rules themselves are allowed to be .js.
    files: ["**/*.config.{js,ts}", "eslint-rules/**/*.js"],
    languageOptions: { globals: globals.node },
    rules: {
      "local/no-js-file-extension": "off",
    },
  },

  // Prettier last so it wins format disagreements.
  prettier,
];
