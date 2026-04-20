// Custom confirm dialog. Replaces window.confirm everywhere — native
// browser popups block the event loop, steal focus from xterm, and
// render inconsistently across Chromium / Firefox / Safari on LAN
// origins. Rule: no window.confirm / window.alert / window.prompt
// anywhere in the app.

import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";

import "./ConfirmDialog.css";

export interface ConfirmDialogProps {
  title: string;
  message: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** If true, the confirm button is rendered red. Defaults to false. */
  destructive?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({
  title,
  message,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  destructive = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const confirmRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel();
      } else if (e.key === "Enter") {
        e.preventDefault();
        onConfirm();
      }
    };
    window.addEventListener("keydown", onKey);
    // Focus the confirm button on mount so Enter = confirm works
    // without an extra click.
    confirmRef.current?.focus();
    return () => window.removeEventListener("keydown", onKey);
  }, [onConfirm, onCancel]);

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
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
