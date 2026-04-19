import { ContextMenuProvider } from "./components/common/ContextMenu";
import { Layout } from "./components/Layout";
import { RepoProvider } from "./state/RepoStore";
import { SessionProvider } from "./state/SessionStore";
import { TabProvider } from "./state/TabStore";

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
