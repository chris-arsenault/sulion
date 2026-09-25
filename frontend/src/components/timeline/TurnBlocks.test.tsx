import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { VirtuosoMockContext } from "react-virtuoso";
import { TurnBlocks } from "./TurnBlocks";
import { groupItems, type TurnBlock } from "./turnDetailCache";

const dimensions = { viewportHeight: 600, itemHeight: 80 };
const blocks: TurnBlock[] = Array.from({ length: 1000 }, (_, index) => ({ kind: "tool", pair_id: `tool-${index}` }));
const textBlocks = groupItems(Array.from({ length: 4096 }, (_, index) => ({
  offset: index, kind: "assistant" as const, items: [{ kind: "text" as const, text: `message ${index}` }], thinking: [],
})));
const renderBlock = (block: TurnBlock) => <div data-testid="rendered-block">{block.kind === "tool" ? block.pair_id : "text"}</div>;

describe("long turn viewport", () => {
  it("virtualizes a text-only turn even when grouping yields fewer than a hundred blocks", async () => {
    render(<VirtuosoMockContext.Provider value={dimensions}>
      <TurnBlocks blocks={textBlocks} scroller={null} focusPairId={null} focusKey={null}>{renderBlock}</TurnBlocks>
    </VirtuosoMockContext.Provider>);
    await waitFor(() => expect(screen.getAllByTestId("rendered-block").length).toBeGreaterThan(1));
    expect(screen.getAllByTestId("rendered-block").length).toBeLessThan(30);
  });
  it("mounts only a bounded viewport and overscan for a thousand blocks", async () => {
    render(<VirtuosoMockContext.Provider value={dimensions}>
      <TurnBlocks blocks={blocks} scroller={null} focusPairId={null} focusKey={null}>{renderBlock}</TurnBlocks>
    </VirtuosoMockContext.Provider>);
    await waitFor(() => expect(screen.getAllByTestId("rendered-block").length).toBeGreaterThan(1));
    expect(screen.getAllByTestId("rendered-block").length).toBeLessThan(30);
  });
});
