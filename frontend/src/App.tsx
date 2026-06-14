import { AuthGate, AuthProvider } from "./auth/AuthProvider";
import { ContextMenuHost } from "./components/common/ContextMenu";
import { Layout } from "./components/Layout";
import { PairPage } from "./components/PairPage";

// No router library — the app is a single view. The one standalone page is the
// device-pairing approval page at `/pair`, served inside the same AuthGate so it
// carries the logged-in user's session.
function isPairRoute(): boolean {
  return typeof window !== "undefined" && window.location.pathname === "/pair";
}

export function App() {
  return (
    <AuthProvider>
      <AuthGate>
        {isPairRoute() ? (
          <PairPage />
        ) : (
          <>
            <Layout />
            <ContextMenuHost />
          </>
        )}
      </AuthGate>
    </AuthProvider>
  );
}
