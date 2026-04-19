# Local ESLint rules

Small, project-specific rules that encode decisions the plugins can't
express. Ported from
[ahara-shell](https://github.com/chris-arsenault/athena-s3-web-shell/tree/main/eslint-rules).

| Rule | Enforces |
|---|---|
| `max-jsx-props` | Cap JSX prop count. Past ~12–14 props, refactor to a single props object. Warning. |
| `no-direct-fetch` | `fetch()` calls must go through `src/api/client.ts` — or, for browser-side streaming parsers, `*.worker.ts`. Keeps HTTP auth / base URLs / error shapes centralised. Error. |
| `no-inline-styles` | `style={{…}}` is forbidden. Use a co-located `.css` class. For genuinely dynamic values (computed widths, grid fractions), add `// eslint-disable-next-line local/no-inline-styles` with a comment. Warning. |
| `no-js-file-extension` | Source files are `.ts` / `.tsx`. `.js` is only allowed in `eslint-rules/` and for config files. Error. |
| `no-non-vitest-testing` | Only Vitest. Importing `jest`, `mocha`, etc. is blocked. Error. |
| `no-raw-undefined-union` | Discourage raw `T \| undefined` in annotations; prefer `?:` or a named alias. Off by default — enable per-module when doing a type cleanup. |

Severity levels are set in `eslint.config.js`. The "load-bearing" rules
(no-direct-fetch, no-non-vitest-testing, no-js-file-extension) run as
errors; the style-ish ones run as warnings so pre-existing violations
don't block CI.
