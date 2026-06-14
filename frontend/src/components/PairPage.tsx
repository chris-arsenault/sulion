// Standalone `/pair` page: the browser half of the device-pairing flow.
// An external extension (e.g. the Ableton "Send to Sulion" extension) opens
// `${SULION_PUBLIC_URL}/pair?code=WXYZ-1234`; the logged-in user confirms the
// code matches what the device shows and approves. The device polls
// `/api/devices/pair/token` separately and gets its token once this succeeds.
//
// Rendered by App.tsx for `location.pathname === "/pair"`, inside the same
// AuthGate as the rest of the app — so the approve call carries the user's
// Cognito session and the backend binds the pairing to that identity.

import { useCallback, useState } from "react";

import { ApiError, approveDevicePairing } from "../api/client";
import "./PairPage.css";

type Phase =
  | { kind: "idle" }
  | { kind: "submitting" }
  | { kind: "approved"; client: string }
  | { kind: "error"; message: string };

function initialCode(): string {
  if (typeof window === "undefined") return "";
  const raw = new URLSearchParams(window.location.search).get("code") ?? "";
  return raw.trim().toUpperCase();
}

export function PairPage() {
  const [code, setCode] = useState(initialCode);
  const [phase, setPhase] = useState<Phase>({ kind: "idle" });

  const normalized = code.trim().toUpperCase();
  const canSubmit = normalized.length > 0 && phase.kind !== "submitting";

  const approve = useCallback(async () => {
    setPhase({ kind: "submitting" });
    try {
      const res = await approveDevicePairing(normalized);
      setPhase({ kind: "approved", client: res.client });
    } catch (err) {
      setPhase({ kind: "error", message: errorMessage(err) });
    }
  }, [normalized]);

  const onSubmit = useCallback(
    (ev: React.FormEvent<HTMLFormElement>) => {
      ev.preventDefault();
      if (canSubmit) void approve();
    },
    [approve, canSubmit],
  );

  const onCodeChange = useCallback(
    (ev: React.ChangeEvent<HTMLInputElement>) => {
      setCode(ev.target.value.toUpperCase());
    },
    [],
  );

  if (phase.kind === "approved") {
    return (
      <div className="pair-shell">
        <div className="pair-card" data-testid="pair-approved">
          <div className="pair-card__eyebrow">sulion</div>
          <h1 className="pair-card__title">Device approved</h1>
          <p className="pair-card__copy">
            <strong>{phase.client}</strong> can now send to Sulion. You can close
            this tab and return to the device.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="pair-shell">
      <form className="pair-card" onSubmit={onSubmit}>
        <div className="pair-card__eyebrow">sulion</div>
        <h1 className="pair-card__title">Approve device</h1>
        <p className="pair-card__copy">
          A device is requesting access to send data to Sulion. Confirm the code
          below matches the one shown on the device, then approve.
        </p>

        <label className="pair-card__field">
          <span>Pairing code</span>
          <input
            className="pair-card__code"
            value={code}
            onChange={onCodeChange}
            autoFocus
            autoComplete="off"
            autoCapitalize="characters"
            spellCheck={false}
            placeholder="WXYZ-1234"
            data-testid="pair-code-input"
            disabled={phase.kind === "submitting"}
          />
        </label>

        {phase.kind === "error" && (
          <div className="pair-card__error" role="alert">
            {phase.message}
          </div>
        )}

        <button
          type="submit"
          className="pair-card__submit"
          disabled={!canSubmit}
          data-testid="pair-approve"
        >
          {phase.kind === "submitting" ? "approving…" : "Approve"}
        </button>
      </form>
    </div>
  );
}

function errorMessage(err: unknown): string {
  if (err instanceof ApiError) return err.message;
  if (err instanceof Error) return err.message;
  return "approval failed";
}
