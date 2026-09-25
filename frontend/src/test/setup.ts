import { afterEach, beforeEach } from "vitest";
import { cleanup } from "@testing-library/react";

import { resetContextMenuStore } from "../components/common/contextMenuStore";
import { resetAppCommands } from "../state/AppCommands";
import { resetRepoStore } from "../state/RepoStore";
import { resetSecretStore } from "../state/SecretStore";
import { resetSessionStore } from "../state/SessionStore";
import { resetTabStore } from "../state/TabStore";
import { resetTurnDetails } from "../state/TurnDetailStore";

function resetAllStores() {
  resetTurnDetails();
  resetAppCommands();
  resetContextMenuStore();
  resetRepoStore();
  resetSecretStore();
  resetSessionStore();
  resetTabStore();
}

beforeEach(() => {
  window.localStorage.clear();
  window.history.replaceState({}, "", "/");
  window.__APP_CONFIG__ = undefined;
  resetAllStores();
});

afterEach(() => {
  cleanup();
  resetAllStores();
});
