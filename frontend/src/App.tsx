import { ContextMenuProvider } from "./components/common/ContextMenu";
import { Layout } from "./components/Layout";
import { RepoProvider } from "./state/RepoStore";
import { SessionProvider } from "./state/SessionStore";

// TabStore is a Zustand store; no provider wrap needed. SessionProvider
// and RepoProvider are still React contexts — migrating them is a
// follow-up once the TabStore pattern has a couple of weeks of use.

export function App() {
  return (
    <SessionProvider>
      <RepoProvider>
        <ContextMenuProvider>
          <Layout />
        </ContextMenuProvider>
      </RepoProvider>
    </SessionProvider>
  );
}
