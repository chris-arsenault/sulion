# Permanent repository secret grants

Outcome: allow an authenticated user to enable a secret indefinitely for one
repository, affecting existing and future sessions, and revoke that access.
Implementation, validation, commit and push are authorized.

The broker owns encrypted values and grants. Reuse its browser-authenticated
grant API, signed redemption, and shared terminal/session context menu. Store
repository grants separately from timed grants in the existing grants table,
with constraints requiring exactly one scope. Repository grants have no expiry.
Keep both grants visible when a secret has both scopes, and deduplicate only
at redemption. Revoking one scope must not silently revoke the other.

Repository identity comes from the node's persisted session metadata at key
registration and adoption, never from the signed wrapper request or cwd.
An unknown repository cannot receive permanent access. Collection sessions use
their registered primary repository, not the union of collection members.
Repository names are the existing identity; rename does not transfer grants.
The existing shared-uid limitation documented in secrets.md still applies.

Alternatives rejected: an arbitrarily distant expiry is not permanent; copying
grants into each PTY loses repository-wide revocation and future-session access.

## M0 — Repository grants end to end

Acceptance: persistent grants authorize two sessions of the same repository,
deny another repository, survive broker state reconstruction, and stop after
revocation. Timed grants, signed requests, env conflicts and value concealment
retain their contracts. Menus distinguish scope and show permanent enable/revoke.

Root: `7b329adc-5cfe-4d57-8340-3314f53dc839`.
Milestone: `087051c2-bab5-4c5f-9126-130c0d296821`.
Expansion: `4f8bfead-f9dc-4aa3-b1c9-3cb6aada1108`, steps 1 and 2.

### Execution steps

1. Broker and node: migration, grant validation/list/redeem/revoke in
   `backend/src/secret_broker.rs`, registration protocol and node lifecycle.
   Verify through isolated Postgres broker integration tests with signed use.
2. Browser and docs: typed API/store and shared context menu, scope-specific
   revoke, refresh affected sessions. Verify menu/store tests, typecheck and
   repository checks; update `docs/secrets.md` and changelog.

Current state: broker, registration/adoption, browser and docs implemented.
Browser step complete: 62 files / 398 tests pass; TypeScript checking and ESLint
pass (five existing warnings outside the changed code). The store test initially
mocked a module already loaded by global setup; it now stubs fetch and exercises
the real API serialization. Menu, scope deduplication and cross-session refresh
regressions pass. Rust clippy, structure lint, formatting and all 304 unit tests
pass. The final isolated Postgres broker regression passes: persistence, future
sessions, adoption, cross-repository denial, replay and signature rejection,
conflicts, independent revocation, expiry and concealed browser values.
Tests use a separate temporary broker database so its
migration ledger cannot collide with the main database's migration ledger.
Browser E2E requires explicit server-start authorization and is not an exit gate.

Final evidence: `make ci` passed, including deploy rendering, clippy, structure
checks, formatting, 304 Rust unit tests, doc tests, TypeScript checking, lint
and 398 frontend tests. The final
`./scripts/run-backend-integration-tests.sh secret_broker_integration` passed.
`git diff --check` passed. Implementation is complete and approved for commit
and push. No deployed grant changes were performed. Browser E2E was not run.
