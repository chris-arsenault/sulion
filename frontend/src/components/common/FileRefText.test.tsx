import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import {
  type AppCommand,
  resetAppCommands,
  subscribeToAppCommand,
} from "../../state/AppCommands";
import { FileRefText } from "./FileRefText";

describe("FileRefText", () => {
  afterEach(() => resetAppCommands());

  it("linkifies path:line references and passes plain text through", () => {
    render(
      <FileRefText
        text="Wire the store in frontend/src/state/TabStore.tsx:441 before the UI."
        repo="alpha"
      />,
    );
    const ref = screen.getByRole("button", {
      name: "frontend/src/state/TabStore.tsx:441",
    });
    expect(ref).toBeDefined();
    expect(screen.getByText(/Wire the store in/)).toBeDefined();
    expect(screen.getByText(/before the UI\./)).toBeDefined();
  });

  it("opens the referenced file at its line when clicked", async () => {
    const commands: AppCommand[] = [];
    resetAppCommands();
    subscribeToAppCommand("open-file", (command) => commands.push(command));

    render(<FileRefText text="See src/app.ts:12:5 here." repo="alpha" />);
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "src/app.ts:12:5" }));

    expect(commands).toEqual([
      {
        type: "open-file",
        repo: "alpha",
        path: "src/app.ts",
        workspaceId: undefined,
        line: 12,
      },
    ]);
  });

  it("does not treat prose colons or URLs as file references", () => {
    render(
      <FileRefText
        text="TODO:12 and a link https://example.com:8080/x — nothing here."
        repo="alpha"
      />,
    );
    expect(screen.queryByRole("button")).toBeNull();
  });
});
