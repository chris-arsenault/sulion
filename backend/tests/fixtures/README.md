`codex-rich-lineage-parent.jsonl` and `codex-rich-lineage-child.jsonl` are trimmed from a real local Codex rollout captured on `2026-04-19`.

They intentionally keep only the lineage-bearing fields needed by the ingester tests:

- `session_meta.id`
- `session_meta.forked_from_id`
- `session_meta.source.subagent.thread_spawn.*`
- `turn_context.turn_id`
- `event_msg.task_started.turn_id`
- `event_msg.collab_agent_spawn_end.{call_id,sender_thread_id,new_thread_id}`
- `response_item.function_call.call_id`

The source rollouts were:

- `~/.codex/sessions/2026/04/19/rollout-2026-04-19T04-13-03-019da571-ab6d-72e2-94b2-4fc5544f53d2.jsonl`
- `~/.codex/sessions/2026/04/19/rollout-2026-04-19T13-58-36-019da789-c2a6-7f80-b71b-4dc90c7f1802.jsonl`
# Authentication fixture

`auth-test-private.pem` and `auth-test-jwks.json` are a generated RSA test keypair.
They are public test data, never deployment credentials. Auth unit tests sign
tokens for a disposable loopback issuer with them; production accepts only its
configured Cognito issuer and keys.
