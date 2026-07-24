import {
  type ChangeEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";

import { getAuthRuntimeConfig } from "./config";
import {
  getSessionSnapshot,
  isAuthConfigured,
  signIn,
  signOut,
  type SignInResult,
} from "./cognito";
import {
  AuthContext,
  type AuthContextValue,
  type AuthState,
  useAuth,
} from "./context";
import "./AuthProvider.css";

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>({ kind: "loading" });

  useEffect(() => {
    let cancelled = false;
    if (!isAuthConfigured()) {
      setState({ kind: "disabled" });
      return;
    }
    void (async () => {
      try {
        const session = await getSessionSnapshot();
        if (cancelled) return;
        if (session) setState({ kind: "authenticated", session });
        else setState({ kind: "anonymous", error: null });
      } catch (err) {
        if (cancelled) return;
        setState({
          kind: "anonymous",
          error: err instanceof Error ? err.message : "authentication failed",
        });
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const value = useMemo<AuthContextValue>(() => {
    const applyResult = (result: SignInResult) => {
      if (result.kind === "authenticated") {
        setState({ kind: "authenticated", session: result.session });
      } else {
        setState({ kind: "mfa", challenge: result.challenge, error: null, pending: false });
      }
    };
    return {
      state,
      async login(username: string, password: string) {
        setState({ kind: "loading" });
        try {
          applyResult(await signIn(username, password));
        } catch (err) {
          setState({
            kind: "anonymous",
            error: err instanceof Error ? err.message : "login failed",
          });
        }
      },
      async submitMfaCode(code: string) {
        if (state.kind !== "mfa") return;
        const { challenge } = state;
        setState({ kind: "mfa", challenge, error: null, pending: true });
        try {
          applyResult(await challenge.submitCode(code));
        } catch (err) {
          setState({
            kind: "mfa",
            challenge,
            error: err instanceof Error ? err.message : "verification failed",
            pending: false,
          });
        }
      },
      cancelMfa() {
        signOut();
        setState({ kind: "anonymous", error: null });
      },
      logout() {
        signOut();
        setState({ kind: "anonymous", error: null });
      },
    };
  }, [state]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function AuthGate({ children }: { children: ReactNode }) {
  const { state, login, submitMfaCode, cancelMfa, logout } = useAuth();
  const runtime = getAuthRuntimeConfig();

  if (state.kind === "loading") {
    return <div className="auth-shell auth-shell--loading">checking session…</div>;
  }

  if (state.kind === "disabled") {
    return <>{children}</>;
  }

  if (state.kind === "mfa") {
    return (
      <MfaScreen
        error={state.error}
        pending={state.pending}
        onSubmit={submitMfaCode}
        onCancel={cancelMfa}
      />
    );
  }

  if (state.kind === "authenticated") {
    return (
      <div className="auth-app">
        <div className="auth-app__status">
          <span className="auth-app__user">
            {state.session.username ?? state.session.email ?? "signed in"}
          </span>
          <button type="button" className="auth-app__logout" onClick={logout}>
            sign out
          </button>
        </div>
        {children}
      </div>
    );
  }

  return (
    <LoginScreen
      configured={runtime != null}
      error={state.error}
      onLogin={login}
    />
  );
}

function LoginScreen({
  configured,
  error,
  onLogin,
}: {
  configured: boolean;
  error: string | null;
  onLogin: (username: string, password: string) => Promise<void>;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [submitting, setSubmitting] = useState(false);

  const submit = useCallback(
    async (ev: React.FormEvent<HTMLFormElement>) => {
      ev.preventDefault();
      setSubmitting(true);
      try {
        await onLogin(username, password);
      } finally {
        setSubmitting(false);
      }
    },
    [onLogin, password, username],
  );
  const onUsernameChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setUsername(event.target.value),
    [],
  );
  const onPasswordChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setPassword(event.target.value),
    [],
  );

  return (
    <div className="auth-shell">
      <form className="auth-card" onSubmit={submit}>
        <div className="auth-card__eyebrow">sulion</div>
        <h1 className="auth-card__title">Authenticate to continue</h1>
        <p className="auth-card__copy">
          The frontend, REST API, and PTY websocket require a shared Ahara Cognito session.
        </p>
        {!configured && (
          <div className="auth-card__error">
            Cognito runtime config is missing. Set `cognitoUserPoolId` and `cognitoClientId`.
          </div>
        )}
        {error && <div className="auth-card__error">{error}</div>}
        <label className="auth-card__field">
          <span>Username or email</span>
          <input
            type="text"
            autoComplete="username"
            value={username}
            onChange={onUsernameChange}
            disabled={!configured || submitting}
          />
        </label>
        <label className="auth-card__field">
          <span>Password</span>
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            onChange={onPasswordChange}
            disabled={!configured || submitting}
          />
        </label>
        <button
          type="submit"
          className="auth-card__submit"
          disabled={!configured || submitting || username.trim() === "" || password === ""}
        >
          {submitting ? "signing in…" : "sign in"}
        </button>
      </form>
    </div>
  );
}

function MfaScreen({
  error,
  pending,
  onSubmit,
  onCancel,
}: {
  error: string | null;
  pending: boolean;
  onSubmit: (code: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [code, setCode] = useState("");

  const submit = useCallback(
    async (ev: React.FormEvent<HTMLFormElement>) => {
      ev.preventDefault();
      await onSubmit(code);
      setCode("");
    },
    [code, onSubmit],
  );
  const onCodeChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) =>
      setCode(event.target.value.replace(/\D/g, "")),
    [],
  );

  return (
    <div className="auth-shell">
      <form className="auth-card" onSubmit={submit}>
        <div className="auth-card__eyebrow">sulion</div>
        <h1 className="auth-card__title">Two-factor authentication</h1>
        <p className="auth-card__copy">
          Enter the current 6-digit code from your authenticator app. Don't have one
          enrolled yet? Set it up in the Ahara account portal, then sign in here.
        </p>
        {error && <div className="auth-card__error">{error}</div>}
        <label className="auth-card__field">
          <span>Authenticator code</span>
          <input
            type="text"
            inputMode="numeric"
            autoComplete="one-time-code"
            pattern="[0-9]*"
            maxLength={6}
            value={code}
            onChange={onCodeChange}
            disabled={pending}
            autoFocus
          />
        </label>
        <button
          type="submit"
          className="auth-card__submit"
          disabled={pending || code.length < 6}
        >
          {pending ? "verifying…" : "verify"}
        </button>
        <button
          type="button"
          className="auth-card__link auth-card__link--button"
          onClick={onCancel}
          disabled={pending}
        >
          back to sign in
        </button>
      </form>
    </div>
  );
}
