import { afterEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { FilterChips } from "./FilterChips";
import { DEFAULT_FILTERS, useTimelineFilters } from "./filters";

const ARIA_PRESSED = "aria-pressed";

function Host() {
  const hook = useTimelineFilters();
  return <FilterChips {...hook} />;
}

describe("FilterChips — exclusion UI", () => {
  afterEach(() => window.localStorage.clear());

  it("renders speaker, operation, include, and file-path chips", () => {
    render(<Host />);
    expect(screen.getByRole("button", { name: /^user$/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /^assistant$/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /tool result/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /create content/i })).toBeDefined();
    expect(screen.getByRole("button", { name: /errors only/i })).toBeDefined();
    expect(screen.getByPlaceholderText(/path/i)).toBeDefined();
  });

  it("clicking a category chip marks it as hidden (aria-pressed + hidden class)", async () => {
    const user = userEvent.setup();
    render(<Host />);
    const createChip = screen.getByRole("button", { name: /create content/i });
    expect(createChip.getAttribute(ARIA_PRESSED)).toBe("false");
    expect(createChip.className).toContain("visible");
    await user.click(createChip);
    expect(createChip.getAttribute(ARIA_PRESSED)).toBe("true");
    expect(createChip.className).toContain("hidden");
  });

  it("hide toggles persist to localStorage", async () => {
    const user = userEvent.setup();
    render(<Host />);
    const userChip = screen.getByRole("button", { name: /^user$/i });
    await user.click(userChip);

    const stored = window.localStorage.getItem(
      "sulion.timeline.filters.v3",
    );
    expect(stored).not.toBeNull();
    const parsed = JSON.parse(stored!);
    expect(parsed.hiddenSpeakers).toContain("user");
  });

  it("Show all button appears when anything is hidden, resets state when clicked", async () => {
    const user = userEvent.setup();
    render(<Host />);
    expect(screen.queryByRole("button", { name: /show all/i })).toBeNull();
    await user.click(screen.getByRole("button", { name: /create content/i }));
    const clear = screen.getByRole("button", { name: /show all/i });
    await user.click(clear);
    const createChip = screen.getByRole("button", { name: /create content/i });
    expect(createChip.getAttribute(ARIA_PRESSED)).toBe("false");
  });

  it("typing in the file-path input updates state", async () => {
    const user = userEvent.setup();
    render(<Host />);
    const input = screen.getByPlaceholderText(/path/i);
    await user.type(input, "foo");
    expect((input as HTMLInputElement).value).toBe("foo");
  });

  it("errors-only is an include chip (dim default, bright when active)", async () => {
    const user = userEvent.setup();
    render(<Host />);
    const chip = screen.getByRole("button", { name: /errors only/i });
    expect(chip.getAttribute(ARIA_PRESSED)).toBe("false");
    await user.click(chip);
    expect(chip.getAttribute(ARIA_PRESSED)).toBe("true");
    expect(chip.className).toContain("include-active");
  });

  it("default state: thinking visible, bookkeeping hidden, sidechain hidden", () => {
    expect(DEFAULT_FILTERS.showThinking).toBe(true);
    expect(DEFAULT_FILTERS.showBookkeeping).toBe(false);
    expect(DEFAULT_FILTERS.showSidechain).toBe(false);
  });
});
