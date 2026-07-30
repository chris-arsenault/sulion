#!/usr/bin/env bash
#
# Claude lifecycle hook for Sulion's PTY activity projection. This is
# best-effort observability only: hook failures must never stop Claude.

set -u

if [[ -z "${SULION_PTY_ID:-}" ]]; then
  exit 0
fi

PAYLOAD="$(cat 2>/dev/null || true)"
EVENT="$(printf '%s' "${PAYLOAD}" | jq -r '.hook_event_name // empty' 2>/dev/null)"

case "${EVENT}" in
  UserPromptSubmit)
    PROMPT="$(printf '%s' "${PAYLOAD}" | jq -r '.prompt // empty' 2>/dev/null)"
    timeout 2s /usr/local/bin/sulion activity working --summary "${PROMPT}" \
      >/dev/null 2>&1 || true
    ;;
  PreToolUse)
    TOOL="$(printf '%s' "${PAYLOAD}" | jq -r '.tool_name // empty' 2>/dev/null)"
    timeout 2s /usr/local/bin/sulion activity working --summary "${TOOL}" \
      >/dev/null 2>&1 || true
    ;;
  Stop)
    MESSAGE="$(printf '%s' "${PAYLOAD}" | jq -r '.last_assistant_message // empty' 2>/dev/null)"
    timeout 2s /usr/local/bin/sulion activity awaiting --summary "${MESSAGE}" \
      >/dev/null 2>&1 || true
    ;;
  Notification)
    TYPE="$(printf '%s' "${PAYLOAD}" | jq -r '.notification_type // empty' 2>/dev/null)"
    if [[ "${TYPE}" == "permission_prompt" || "${TYPE}" == "elicitation_dialog" ]]; then
      MESSAGE="$(printf '%s' "${PAYLOAD}" | jq -r '.message // .title // empty' 2>/dev/null)"
      timeout 2s /usr/local/bin/sulion activity waiting --reason "${MESSAGE}" \
        >/dev/null 2>&1 || true
    fi
    ;;
esac

exit 0
