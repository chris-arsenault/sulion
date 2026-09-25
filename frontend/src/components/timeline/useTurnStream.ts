import { useEffect } from "react";
import { detailKey, refreshTurn, retainTurn, useTurnDetailStore } from "../../state/TurnDetailStore";

export function useTurnStream(session: string | null, turn: number | null, revision: number, active = true) {
  const key = session != null && turn != null ? detailKey(session, turn) : null;
  const entry = useTurnDetailStore((state) => key ? state.entries.get(key) : undefined);
  useEffect(() => {
    if (session == null || turn == null || !active) return;
    return retainTurn(session, turn);
  }, [session, turn, active]);
  useEffect(() => {
    if (session != null && turn != null && active) refreshTurn(session, turn, revision);
  }, [session, turn, revision, active]);
  return entry;
}
