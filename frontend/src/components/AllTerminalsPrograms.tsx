import { type ChangeEvent, useCallback, useState } from "react";

/**
 * Edits a secret's every-terminal grant: the programs `with-cred` injects it
 * into in every terminal. Remount with a new key when the stored list changes.
 */
export function AllTerminalsPrograms({
  programs,
  disabled,
  onApply,
}: {
  programs: string[] | null;
  disabled: boolean;
  onApply: (programs: string[] | null) => Promise<void>;
}) {
  const [draft, setDraft] = useState((programs ?? []).join("\n"));
  const [busy, setBusy] = useState(false);
  const parsed = draft
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);

  const apply = useCallback(
    async (next: string[] | null) => {
      setBusy(true);
      try {
        await onApply(next);
      } finally {
        setBusy(false);
      }
    },
    [onApply],
  );
  const onDraftChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setDraft(event.target.value),
    [],
  );
  const grant = useCallback(() => apply(parsed), [apply, parsed]);
  const revoke = useCallback(() => apply(null), [apply]);

  return (
    <div className="secrets-tab__all-terminals">
      <label className="secrets-tab__field">
        <span>Every-terminal programs</span>
        <textarea
          className="secrets-tab__input--mono"
          value={draft}
          onChange={onDraftChange}
          rows={2}
        />
      </label>
      <span className="secrets-tab__field-hint">
        `with-cred` injects this secret in every terminal only into these
        programs (command names, one per line). Any other command still needs a
        terminal or repository grant, which also replaces this value.
      </span>
      <div className="secrets-tab__all-terminals-actions">
        <button
          type="button"
          className="secrets-tab__button"
          onClick={grant}
          disabled={disabled || busy || parsed.length === 0}
        >
          {programs ? "Update programs" : "Enable for every terminal"}
        </button>
        {programs ? (
          <button
            type="button"
            className="secrets-tab__button secrets-tab__button--danger"
            onClick={revoke}
            disabled={disabled || busy}
          >
            Disable for every terminal
          </button>
        ) : null}
      </div>
    </div>
  );
}
