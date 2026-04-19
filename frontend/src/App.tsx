import { ContextMenuProvider } from "./components/common/ContextMenu";
import { Layout } from "./components/Layout";
import { RepoProvider } from "./state/RepoStore";
import { SessionProvider } from "./state/SessionStore";
import { TabProvider } from "./state/TabStore";

// All stores on React context. The TabStore registry is thin enough
// (id + kind + refs + pane membership) that context re-render cost is
// negligible. See docs/state-management.md for the decision.

export function App() {
  return (
    <SessionProvider>
      <RepoProvider>
        <TabProvider>
          <ContextMenuProvider>
            <Layout />
          </ContextMenuProvider>
        </TabProvider>
      </RepoProvider>
    </SessionProvider>
  );
}
