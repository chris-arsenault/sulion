import { useCallback, useEffect, useState } from "react";

import {
  deleteSecret,
  getSecret,
  listSecrets,
  upsertSecret,
} from "../api/client";
import type { SecretEnvelope, SecretMetadata } from "../api/types";
import { Icon } from "../icons";
import { useSecretStore } from "../state/SecretStore";
import "./SecretsTab.css";

const EMPTY_SECRET: SecretEnvelope = {
  description: "",
  scope: "global",
  repo: null,
  env: { EXAMPLE_KEY: "" },
};

export function SecretsTab() {
  const refreshSecretStore = useSecretStore((store) => store.refreshSecrets);
  const [secrets, setSecrets] = useState<SecretMetadata[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draftId, setDraftId] = useState("");
  const [draft, setDraft] = useState<SecretEnvelope>(EMPTY_SECRET);
  const [loading, setLoading] = useState(true);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const loadSecrets = useCallback(async () => {
    setLoading(true);
    try {
      const items = await listSecrets();
      setSecrets(items);
      setSelectedId((prev) => prev ?? items[0]?.id ?? null);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "secret load failed");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSecrets();
  }, [loadSecrets]);

  useEffect(() => {
    if (!selectedId) {
      setDraftId("");
      setDraft(EMPTY_SECRET);
      return;
    }
    setLoadingDetail(true);
    void getSecret(selectedId)
      .then((secret) => {
        setDraftId(selectedId);
        setDraft(secret);
        setError(null);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : "secret load failed");
      })
      .finally(() => setLoadingDetail(false));
  }, [selectedId]);

  const startNew = useCallback(() => {
    setSelectedId(null);
    setDraftId("");
    setDraft(EMPTY_SECRET);
    setNotice(null);
    setError(null);
  }, []);

  const saveSecret = useCallback(async () => {
    const id = draftId.trim();
    const env = trimEnvMap(draft.env);
    if (!id) {
      setError("secret id is required");
      return;
    }
    if (Object.keys(env).length === 0) {
      setError("at least one env entry is required");
      return;
    }
    if (!selectedId && Object.values(env).every((value) => value === "")) {
      setError("at least one secret value is required for a new bundle");
      return;
    }
    setSaving(true);
    try {
      await upsertSecret(id, {
        description: draft.description.trim(),
        scope: draft.scope.trim() || "global",
        repo: draft.repo?.trim() || null,
        env,
      });
      await loadSecrets();
      await refreshSecretStore();
      setSelectedId(id);
      setNotice(`Saved ${id}`);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "secret save failed");
    } finally {
      setSaving(false);
    }
  }, [draft, draftId, loadSecrets, refreshSecretStore, selectedId]);

  const removeSecret = useCallback(async () => {
    if (!draftId) return;
    try {
      await deleteSecret(draftId);
      await loadSecrets();
      await refreshSecretStore();
      setNotice(`Deleted ${draftId}`);
      setSelectedId(null);
      setDraftId("");
      setDraft(EMPTY_SECRET);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "secret delete failed");
    }
  }, [draftId, loadSecrets, refreshSecretStore]);

  return (
    <div className="secrets-tab">
      <aside className="secrets-tab__sidebar">
        <div className="secrets-tab__sidebar-head">
          <div>
            <div className="secrets-tab__eyebrow">Secrets</div>
            <h2>Bundles</h2>
          </div>
          <button type="button" className="secrets-tab__button" onClick={startNew}>
            <Icon name="plus" size={14} />
            <span>New</span>
          </button>
        </div>
        {loading ? (
          <div className="secrets-tab__empty">loading...</div>
        ) : secrets.length === 0 ? (
          <div className="secrets-tab__empty">No secrets yet.</div>
        ) : (
          <ul className="secrets-tab__list">
            {secrets.map((secret) => (
              <li key={secret.id}>
                <button
                  type="button"
                  className={
                    "secrets-tab__list-item" + (secret.id === draftId ? " is-active" : "")
                  }
                  onClick={() => setSelectedId(secret.id)}
                >
                  <span className="secrets-tab__list-title">{secret.id}</span>
                  <span className="secrets-tab__list-meta">{secret.description || secret.scope}</span>
                  <span className="secrets-tab__list-keys">
                    {secret.env_keys.join(", ")}
                  </span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </aside>

      <section className="secrets-tab__main">
        <header className="secrets-tab__header">
          <div>
            <div className="secrets-tab__eyebrow">Secret editor</div>
            <h2>{draftId || "New bundle"}</h2>
          </div>
          <div className="secrets-tab__header-actions">
            {notice ? <span className="secrets-tab__notice">{notice}</span> : null}
            {error ? <span className="secrets-tab__error">{error}</span> : null}
          </div>
        </header>

        <div className="secrets-tab__panel secrets-tab__panel--editor">
          <label className="secrets-tab__field">
            <span>ID</span>
            <input
              value={draftId}
              onChange={(e) => setDraftId(e.target.value)}
              placeholder="claude-api"
              disabled={selectedId != null}
            />
          </label>
          <label className="secrets-tab__field">
            <span>Description</span>
            <input
              value={draft.description}
              onChange={(e) => setDraft((prev) => ({ ...prev, description: e.target.value }))}
              placeholder="Anthropic API key"
            />
          </label>
          <div className="secrets-tab__field-row">
            <label className="secrets-tab__field">
              <span>Scope</span>
              <input
                value={draft.scope}
                onChange={(e) => setDraft((prev) => ({ ...prev, scope: e.target.value }))}
                placeholder="global"
              />
            </label>
            <label className="secrets-tab__field">
              <span>Repo</span>
              <input
                value={draft.repo ?? ""}
                onChange={(e) =>
                  setDraft((prev) => ({
                    ...prev,
                    repo: e.target.value.trim() ? e.target.value : null,
                  }))
                }
                placeholder="optional repo"
              />
            </label>
          </div>

          <div className="secrets-tab__env-head">
            <div>
              <h3>Environment</h3>
              {selectedId ? (
                <span className="secrets-tab__field-hint">
                  Blank existing values are kept; enter a value to overwrite.
                </span>
              ) : null}
            </div>
            <button
              type="button"
              className="secrets-tab__button secrets-tab__button--ghost"
              onClick={() => {
                const key = nextEnvKey(draft.env);
                setDraft((prev) => ({
                  ...prev,
                  env: { ...prev.env, [key]: "" },
                }));
              }}
            >
              <Icon name="plus" size={14} />
              <span>Add pair</span>
            </button>
          </div>
          <div className="secrets-tab__env-list">
            {Object.entries(draft.env).map(([key, value], index) => (
              <div key={index} className="secrets-tab__env-row">
                <input
                  value={key}
                  onChange={(e) => {
                    const nextKey = e.target.value;
                    setDraft((prev) => ({
                      ...prev,
                      env: renameEnvKey(prev.env, key, nextKey),
                    }));
                  }}
                  placeholder="ANTHROPIC_API_KEY"
                />
                <input
                  value={value}
                  onChange={(e) =>
                    setDraft((prev) => ({
                      ...prev,
                      env: { ...prev.env, [key]: e.target.value },
                    }))
                  }
                  placeholder={selectedId ? "keep existing" : "value"}
                />
                <button
                  type="button"
                  className="secrets-tab__icon-button"
                  aria-label={`Remove ${key}`}
                  onClick={() =>
                    setDraft((prev) => ({
                      ...prev,
                      env: removeEnvKey(prev.env, key),
                    }))
                  }
                >
                  <Icon name="x" size={14} />
                </button>
              </div>
            ))}
          </div>
          <div className="secrets-tab__actions">
            <button
              type="button"
              className="secrets-tab__button"
              onClick={() => void saveSecret()}
              disabled={saving || loadingDetail}
            >
              Save
            </button>
            {selectedId ? (
              <button
                type="button"
                className="secrets-tab__button secrets-tab__button--danger"
                onClick={() => void removeSecret()}
              >
                Delete
              </button>
            ) : null}
          </div>
        </div>
      </section>
    </div>
  );
}

function nextEnvKey(env: Record<string, string>) {
  let index = 1;
  while (env[`NEW_KEY_${index}`] !== undefined) index += 1;
  return `NEW_KEY_${index}`;
}

function renameEnvKey(env: Record<string, string>, from: string, to: string) {
  if (from === to || to.trim() === "") return env;
  const next: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    next[key === from ? to : key] = value;
  }
  return next;
}

function removeEnvKey(env: Record<string, string>, keyToRemove: string) {
  const next: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    if (key !== keyToRemove) next[key] = value;
  }
  return next;
}

function trimEnvMap(env: Record<string, string>) {
  const next: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    const trimmedKey = key.trim();
    if (!trimmedKey) continue;
    next[trimmedKey] = value;
  }
  return next;
}
