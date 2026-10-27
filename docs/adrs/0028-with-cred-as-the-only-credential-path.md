# ADR 0028: with-cred Is the Only Credential Path; Grants Are Scoped to a Terminal, a Repository, or Every Terminal Bound to Programs

## Status

Accepted, 2026-10-01.

## Context

ADR 0009 shipped grants keyed by secret, tool, and PTY, with two consumers:
`with-cred` and an `aws` wrapper. On 2026-05-11 the user called the tool
dimension "a meaningless distinction, just extra steps. the relationship
should be cred to pty not cred>tool>pty" (session
`019e18ad-3d5d-7560-8080-81ffc2be088f`, turn 37583); fb3d3d5 dropped it.
Named redemption (`with-cred <id>`) and the `aws` wrapper remained.

Grants were timed only. Every new session in a repository needed the same
secret enabled again, and the user asked for a secret enabled indefinitely
for one repository, affecting existing and future sessions, with revocation
(plan `7b329adc-5cfe-4d57-8340-3314f53dc839`; af371c5, 2026-09-28).

Agents also hit `with-cred -- gh run list` refusing with "secret is not
unlocked" for reads that carried no risk, and the node's CI poller was held
to GitHub's anonymous limit. The user asked for a read-only GitHub credential
"permanently available, with the granted broker secret superseding it for
write access" (session `afcf6efb-d904-40fa-81b8-3716c7de4761`, turn 1929450).

Three attempts followed in quick succession and are recorded as published
plans: command lists on bundles (`f506521a`, then `d2de8d8c`), a `gh` wrapper
with parallel every-terminal routes and a node read route (`94876fa2`), and
the consolidation that landed as 32d8a89 (`2ec52f49`, `e0bee54c`,
`21db941b`).

## Decision

`with-cred -- <command>` is the only credential path. It injects every secret
granted to the terminal plus every all-terminals secret whose program list
names the command, applies precedence, and execs the command. There is no
`with-cred <id>`, no `aws` or `gh` wrapper, no shell-wide export, and
nothing redeems a secret by id.

Grants record an explicit scope on one grant API:

- `terminal`: `pty_session_id` and a TTL of 60–86400 seconds;
- `repository`: the repository the node registered with the PTY's public key
  at launch or adoption; no expiry; revoked repository-wide;
- `all_terminals`: a non-empty list of program names; no terminal,
  repository, or expiry; managed from the Secrets tab.

Terminal and repository grants apply to every program and rank above
all-terminals grants; when both set the same variable the higher rank wins.
Two grants of the same rank setting the same variable are a conflict and the
request is refused. The signed use request carries the program name, and the
helper resolves and signs the canonical executable path. The node's CI poller
registers a repository-less credential and redeems `/v1/use` for program `gh`
exactly as a terminal does, so it receives only the all-terminals secrets
listing `gh` and is audited like a terminal.

## Alternatives considered

- **Grants keyed by tool, and the `aws` wrapper.** The 0009 shape. The tool
  key went in fb3d3d5 at the user's direction (turn 37583); the wrapper went
  in 21db941b, whose outcome is "One supported credential path, bare
  with-cred; no aws wrapper or named-secret code, scripts, or docs."
- **Named-secret redemption `with-cred <id>`.** Removed by the same plan and
  32d8a89. A second redemption form meant two contracts to document and
  secure; `docs/secrets.md` now states that nothing redeems a secret by id.
- **Bundle selection by command.** Plan `f506521a` put a commands list on each
  bundle and refused a command when any matching bundle was locked;
  `d2de8d8c` tried to relax that to "eligible" bundles and was canceled on
  2026-10-01 as superseded. Programs became data on the all-terminals grant
  instead, with no provider rules and no bundle metadata.
- **An arbitrarily distant expiry as "permanent".** Rejected in the
  repository-grants plan: "an arbitrarily distant expiry is not permanent."
- **Copying grants into each PTY.** Rejected there as well: it "loses
  repository-wide revocation and future-session access."
- **Repository identity from the wrapper request or cwd.** Rejected; the
  repository comes from the node's persisted session metadata at key
  registration, so a wrapper cannot choose one, and an unknown repository
  cannot receive permanent access.
- **A `gh` wrapper, a node read route, and parallel every-terminal routes.**
  Built under `94876fa2`, then removed by `2ec52f49` under the user directive
  "extend existing routes, types and queries; add no routes or parallel
  handlers", and by `21db941b`. The node redeems through the ordinary
  registration and `/v1/use` paths.
- **Baking `GH_TOKEN` into PTY environment or deploy configuration.**
  Rejected in `94876fa2`: the token stays in the broker so rotation reaches
  running shells and every use is audited.
- **Ungated every-terminal secrets.** Rejected in `e0bee54c`: "The broker
  refusal is the protection: nothing ambient may make an arbitrary command
  succeed." A terminal without its own grant is still refused for any
  program not on the list.

## Consequences

- Every secret-backed command is `with-cred -- ...`; a refusal exits 66 with
  the broker's reason. Agent instruction templates say so.
- An all-terminals secret is reachable from every PTY, including by a
  prompt-injected agent. It is reserved for credentials whose worst case is
  acceptable everywhere; the program list guards against accidental use, not
  deliberate circumvention.
- Repository grants use the repository name; renaming does not transfer them.
  Collection sessions use their primary repository.
- The rollout kept the signed payload byte-compatible and the helper accepting
  the old `--tool with-cred`, because running shells keep old toolset scripts
  and broker and node deploy independently.
- `git push` over the PTY's SSH key bypasses the broker entirely, so the
  read-only default governs `gh` and API calls, not git. Turn 1929450 names
  HTTPS push through a broker-backed helper as the follow-on; it is not done.

## References

- Credential path and grant model: [`../secrets.md`](../secrets.md)
- Repository grants contract: published plans `7b329adc` and `4f8bfead`;
  the working document's contract is folded into this ADR and
  [`../secrets.md`](../secrets.md)
- Broker origin: [ADR 0009](0009-separate-secret-broker-with-signed-redemption.md)
- CHANGELOG Unreleased, "Repository secrets" and "Repository links and CI
  status"
- Commits: fb3d3d5 (grants to PTY, not tool), af371c5 (repository grants),
  32d8a89 (program-bound every-terminal grants; single path)
- Published plans: `7b329adc` and `4f8bfead` (repository grants), `f506521a`,
  `d2de8d8c` (canceled), `94876fa2`, `e0bee54c`, `2ec52f49`, `21db941b`
- Transcript: `019e18ad-3d5d-7560-8080-81ffc2be088f` turn 37583,
  `afcf6efb-d904-40fa-81b8-3716c7de4761` turn 1929450
