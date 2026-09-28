import { AuthGate, AuthProvider } from "./auth/AuthProvider";
import { ContextMenuHost } from "./components/common/ContextMenu";
import { Layout } from "./components/Layout";

export function App() {
  return (
    <AuthProvider>
      <AuthGate>
        <Layout />
        <ContextMenuHost />
      </AuthGate>
    </AuthProvider>
  );
}
