import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { uploadFile } from "../../api/client";
import { ConfirmDialog } from "./ConfirmDialog";

export type PendingAttachment = {
  repo: string;
  workspaceId?: string;
  sessionId: string;
} & (
  | { kind: "text"; raw: string; size: number; lines: number }
  | { kind: "image"; file: File }
);

export function PasteUploadDialog({ pending, onInsert, onClose }: {
  pending: PendingAttachment;
  onInsert: (text: string) => void;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const file = useMemo(() => pending.kind === "image" ? pending.file
    : new File([pending.raw], `paste-${new Date().toISOString().replace(/[:.]/g, "-")}.txt`, { type: "text/plain" }), [pending]);
  const accept = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await uploadFile(pending, ".sulion-paste", file);
      if (!mounted.current) return;
      onInsert(result.path + " ");
      onClose();
    } catch (reason) {
      if (mounted.current) setError(reason instanceof Error ? reason.message : "Upload failed. Retry or cancel.");
    } finally {
      if (mounted.current) setBusy(false);
    }
  }, [busy, file, onClose, onInsert, pending]);
  const inline = useCallback(() => {
    if (pending.kind === "text" && !busy) { onInsert(pending.raw); onClose(); }
  }, [busy, onClose, onInsert, pending]);
  const description = pending.kind === "text"
    ? `Clipboard is ${file.size} bytes / ${pending.lines} lines. Save it to .sulion-paste/ and insert the installed path?`
    : `Save ${file.name} (${file.size} bytes) to .sulion-paste/ and insert the installed path?`;
  return <ConfirmDialog
    title={pending.kind === "text" ? "Large paste" : "Clipboard image"}
    message={error ? `${description} Upload failed: ${error}` : description}
    busy={busy}
    confirmLabel={busy ? "Uploading and installing…" : error ? "Retry upload" : pending.kind === "image" ? "Upload image" : "Save as file"}
    secondaryConfirmLabel={pending.kind === "text" ? "Paste inline" : undefined}
    onSecondaryConfirm={pending.kind === "text" ? inline : undefined}
    cancelLabel={busy ? "Close" : "Cancel"}
    onConfirm={accept}
    onCancel={onClose}
  />;
}
