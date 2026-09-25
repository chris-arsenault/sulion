import { Fragment, useCallback, useEffect, useRef, type ReactNode } from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { blockKey, type TurnBlock } from "./turnDetailCache";

export function TurnBlocks({ blocks, scroller, focusPairId, focusKey, children }: {
  blocks: TurnBlock[]; scroller: HTMLElement | null; focusPairId: string | null;
  focusKey: string | null; children: (block: TurnBlock, index: number) => ReactNode;
}) {
  const list = useRef<VirtuosoHandle>(null);
  const appliedFocus = useRef<string | null>(null);
  const virtual = blocks.length > 100 || blocks.reduce((count, block) => count + (
    block.kind === "assistant" ? block.items.length + block.thinking.length : 1
  ), 0) > 100;
  const renderItem = useCallback((index: number, block: TurnBlock) => (
    <div className="td__virtual-block">{children(block, index)}</div>
  ), [children]);
  useEffect(() => {
    if (!virtual || !focusKey || !focusPairId || appliedFocus.current === focusKey) return;
    const index = blocks.findIndex((block) => block.kind === "tool" && block.pair_id === focusPairId);
    if (index >= 0) {
      list.current?.scrollToIndex({ index, align: "center" });
      appliedFocus.current = focusKey;
    }
  }, [virtual, blocks, focusPairId, focusKey]);
  if (!virtual) return blocks.map((block, index) => <Fragment key={blockKey(index, block)}>{children(block, index)}</Fragment>);
  return <Virtuoso className="td__virtual-list" ref={list} data={blocks} customScrollParent={scroller ?? undefined}
    computeItemKey={blockKey} itemContent={renderItem} increaseViewportBy={400}
    followOutput={focusKey ? false : "auto"} />;
}
