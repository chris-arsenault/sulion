import { afterEach, beforeEach } from "vitest";
import { cleanup } from "@testing-library/react";

import { resetContextMenuStore } from "../components/common/ContextMenu";
import { resetRepoStore } from "../state/RepoStore";
import { resetSessionStore } from "../state/SessionStore";
import { resetTabStore } from "../state/TabStore";

function resetAllStores() {
  resetContextMenuStore();
  resetRepoStore();
  resetSessionStore();
  resetTabStore();
}

beforeEach(() => {
  window.localStorage.clear();
  window.history.replaceState({}, "", "/");
  resetAllStores();
});

afterEach(() => {
  cleanup();
  resetAllStores();
});
