import { Layout } from "./components/Layout";
import { SessionProvider } from "./state/SessionStore";

export function App() {
  return (
    <SessionProvider>
      <Layout />
    </SessionProvider>
  );
}
