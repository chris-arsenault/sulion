// Custom confirm dialog. Replaces window.confirm everywhere — native
// browser popups block the event loop, steal focus from xterm, and
// render inconsistently across Chromium / Firefox / Safari on LAN
// origins. Rule: no window.confirm / window.alert / window.prompt
// anywhere in the app.

import {
  type ChangeEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import "./ConfirmDialog.css";

export interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel?: string;
  secondaryConfirmLabel?: string;
  cancelLabel?: string;
  /** If true, the confirm button is rendered red. Defaults to false. */
  destructive?: boolean;
  secondaryDestructive?: boolean;
  /** Gate the confirm button behind a typed-phrase match. When set,
   * the dialog shows a text input and Confirm stays disabled until
   * the input exactly equals this value. Used for destructive,
   * hard-to-undo actions (the reindex button). */
  requireText?: string;
  onConfirm: () => void;
  onSecondaryConfirm?: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel = "Confirm",
  secondaryConfirmLabel,
  cancelLabel = "Cancel",
  destructive = false,
  secondaryDestructive = false,
  requireText,
  onConfirm,
  onSecondaryConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const confirmRef = useRef<HTMLButtonElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const [typed, setTyped] = useState("");
  const gated = requireText != null && requireText.length > 0;
  const canConfirm = !gated || typed === requireText;

  const handleConfirm = useCallback(() => {
    if (!canConfirm) return;
    onConfirm();
  }, [canConfirm, onConfirm]);
  const handleSecondaryConfirm = useCallback(() => {
    if (!canConfirm) return;
    onSecondaryConfirm?.();
  }, [canConfirm, onSecondaryConfirm]);
  const handleTypedChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setTyped(event.target.value),
    [],
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      } else if (e.key === "Enter") {
        e.preventDefault();
        handleConfirm();
      }
    };
    window.addEventListener("keydown", onKey);
    // When gated, land the caret in the text input so the user can
    // start typing immediately. Otherwise focus the confirm button
    // so Enter = confirm works without an extra click.
    if (gated) {
      inputRef.current?.focus();
    } else {
      confirmRef.current?.focus();
    }
    return () => window.removeEventListener("keydown", onKey);
  }, [handleConfirm, onCancel, gated]);

  return createPortal(
    <div className="cd__backdrop">
      <button
        type="button"
        className="cd__dismiss"
        aria-label="Dismiss dialog"
        onClick={onCancel}
      />
      <div
        className="cd__content"
        role="dialog"
        aria-modal="true"
        aria-labelledby="cd-title"
      >
        <h3 id="cd-title" className="cd__title">
          {title}
        </h3>
        <p className="cd__message">{message}</p>
        {gated && (
          <label className="cd__require">
            <span className="cd__require-hint">
              Type <code>{requireText}</code> to confirm
            </span>
            <input
              ref={inputRef}
              type="text"
              className="cd__require-input"
              value={typed}
              onChange={handleTypedChange}
              aria-label={`Type ${requireText} to confirm`}
              autoComplete="off"
              spellCheck={false}
            />
          </label>
        )}
        <div className="cd__actions">
          <button type="button" className="cd__btn" onClick={onCancel}>
            {cancelLabel}
          </button>
          <button
            ref={confirmRef}
            type="button"
            className={
              destructive ? "cd__btn cd__btn--destructive" : "cd__btn cd__btn--primary"
            }
            onClick={handleConfirm}
            disabled={!canConfirm}
          >
            {confirmLabel}
          </button>
          {secondaryConfirmLabel && onSecondaryConfirm && (
            <button
              type="button"
              className={
                secondaryDestructive
                  ? "cd__btn cd__btn--destructive"
                  : "cd__btn cd__btn--primary"
              }
              onClick={handleSecondaryConfirm}
              disabled={!canConfirm}
            >
              {secondaryConfirmLabel}
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
