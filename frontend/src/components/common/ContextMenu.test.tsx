import { describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import {
  ContextMenuHost,
  type MenuItem,
  contextMenuHandler,
  useContextMenu,
} from "./ContextMenu";

function Surface({ items }: { items: MenuItem[] }) {
  const open = useContextMenu((store) => store.open);
  const onCtx = contextMenuHandler(open, () => items);
  return (
    <div data-testid="surface" onContextMenu={onCtx} style={{ padding: 40 }}>
      right-click me
    </div>
  );
}

function setup(items: MenuItem[]) {
  return render(
    <>
      <Surface items={items} />
      <ContextMenuHost />
    </>,
  );
}

async function rightClick(el: HTMLElement, user: ReturnType<typeof userEvent.setup>, opts: { shiftKey?: boolean } = {}) {
  await user.pointer({
    keys: opts.shiftKey ? "[MouseRight>{Shift}]" : "[MouseRight]",
    target: el,
  });
}

describe("ContextMenu", () => {
  it("opens on right-click and renders items", async () => {
    setup([
      { kind: "item", id: "a", label: "Alpha", onSelect: vi.fn() },
      { kind: "separator" },
      { kind: "item", id: "b", label: "Beta", onSelect: vi.fn() },
    ]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    expect(screen.getByText("Alpha")).toBeDefined();
    expect(screen.getByText("Beta")).toBeDefined();
  });

  it("invokes onSelect on click and closes", async () => {
    const spy = vi.fn();
    setup([{ kind: "item", id: "hit", label: "Hit", onSelect: spy }]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    await user.click(screen.getByText("Hit"));
    expect(spy).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText("Hit")).toBeNull());
  });

  it("closes on Escape", async () => {
    setup([{ kind: "item", id: "a", label: "Alpha", onSelect: vi.fn() }]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    expect(screen.getByText("Alpha")).toBeDefined();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByText("Alpha")).toBeNull());
  });

  it("shift+right-click falls through to the browser (menu stays closed)", async () => {
    // We can't directly assert "browser default happened" in jsdom, but
    // we can assert our menu did NOT open.
    setup([{ kind: "item", id: "a", label: "Alpha", onSelect: vi.fn() }]);
    const user = userEvent.setup();
    // Use a raw contextmenu event with shiftKey set because userEvent's
    // pointer shim doesn't distinguish the modifier in the synthetic
    // event's shape.
    const el = screen.getByTestId("surface");
    const ev = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
      shiftKey: true,
    });
    el.dispatchEvent(ev);
    // Event is not preventDefault'd and no menu appears.
    expect(ev.defaultPrevented).toBe(false);
    expect(screen.queryByText("Alpha")).toBeNull();
    // sanity for reference
    void user;
  });

  it("opens a submenu on hover and activates nested items", async () => {
    const colour = vi.fn();
    setup([
      {
        kind: "submenu",
        id: "colour",
        label: "Colour",
        items: [
          { kind: "item", id: "red", label: "Red", onSelect: colour },
          { kind: "item", id: "blue", label: "Blue", onSelect: vi.fn() },
        ],
      },
    ]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    await user.hover(screen.getByText("Colour"));
    await waitFor(() => expect(screen.getByText("Red")).toBeDefined());
    await user.click(screen.getByText("Red"));
    expect(colour).toHaveBeenCalledTimes(1);
  });

  it("keyboard nav: ArrowDown / Enter selects an item", async () => {
    const spy = vi.fn();
    setup([
      { kind: "item", id: "a", label: "Alpha", onSelect: vi.fn() },
      { kind: "item", id: "b", label: "Beta", onSelect: spy },
    ]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    // Alpha is auto-focused; arrow down to Beta, enter.
    await user.keyboard("{ArrowDown}");
    await user.keyboard("{Enter}");
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("disabled items do not fire onSelect", async () => {
    const spy = vi.fn();
    setup([
      {
        kind: "item",
        id: "disabled",
        label: "Disabled",
        disabled: true,
        onSelect: spy,
      },
    ]);
    const user = userEvent.setup();
    await rightClick(screen.getByTestId("surface"), user);
    await user.click(screen.getByText("Disabled"));
    expect(spy).not.toHaveBeenCalled();
  });
});
