import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { PlanView } from "../api/types";
import { PlanTree } from "./PlanTree";

function hierarchy(): PlanView[] {
  return Array.from({ length: 9 }, (_, depth) => ({
    id: `plan-${depth}`,
    title: `Depth ${depth}`,
    depth,
    parent_plan_id: depth ? `plan-${depth - 1}` : null,
    root_plan_id: "plan-0",
    repo_name: "atlas",
    status: "active",
    revision: 1,
    summary: "",
    outcome: "",
    principles: [],
    assumptions: [],
    boundaries: [],
    created_by_pty_id: null,
    created_by_agent_session_uuid: null,
    created_at: "2026-11-06T00:00:00Z",
    updated_at: "2026-11-06T00:00:00Z",
    closed_at: null,
    anchor_phase_ids: [],
    ancestors: [],
    branches: [],
    phases: [],
    attachments: [],
  }));
}

describe("PlanTree", () => {
  afterEach(cleanup);

  it("keeps all nine levels, closed ancestors, and selected closed plans in context", async () => {
    const plans = hierarchy();
    plans[0]!.status = "completed";
    plans[8]!.status = "completed";
    const { rerender } = render(
      <PlanTree
        plans={plans}
        selectedPlanId="plan-8"
        showClosed={false}
        onOpenPlan={vi.fn()}
      />,
    );
    const leaf = screen.getByRole("treeitem", { name: "Depth 8" });
    expect(leaf.getAttribute("aria-level")).toBe("9");
    expect(leaf.getAttribute("aria-selected")).toBe("true");
    expect(screen.getAllByRole("treeitem")).toHaveLength(9);
    expect(
      leaf.closest('[role="group"]')?.parentElement?.getAttribute("aria-label"),
    ).toBe("Depth 7");
    rerender(
      <PlanTree
        plans={plans}
        selectedPlanId={undefined}
        showClosed={false}
        onOpenPlan={vi.fn()}
      />,
    );
    expect(screen.queryByRole("treeitem", { name: "Depth 8" })).toBeNull();
    rerender(
      <PlanTree
        plans={plans}
        selectedPlanId={undefined}
        showClosed
        onOpenPlan={vi.fn()}
      />,
    );
    expect(screen.getAllByRole("treeitem")).toHaveLength(9);
  });

  it("collapses branches independently of selection and reveals a newly selected path", async () => {
    const plans = hierarchy();
    const onOpen = vi.fn();
    const { rerender } = render(
      <PlanTree
        plans={plans}
        selectedPlanId="plan-0"
        showClosed={false}
        onOpenPlan={onOpen}
      />,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Collapse Depth 0" }));
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    expect(onOpen).not.toHaveBeenCalled();
    rerender(
      <PlanTree
        plans={plans}
        selectedPlanId="plan-8"
        showClosed={false}
        onOpenPlan={onOpen}
      />,
    );
    await waitFor(() =>
      expect(screen.getAllByRole("treeitem")).toHaveLength(9),
    );
    await user.click(screen.getByRole("button", { name: "Collapse all" }));
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Expand all" }));
    expect(
      screen
        .getByRole("treeitem", { name: "Depth 8" })
        .getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("searches collapsed branches with ancestor context and restores expansion after clearing", async () => {
    const plans = hierarchy();
    plans.push({ ...plans[0]!, id: "unrelated", title: "Other delivery" });
    render(
      <PlanTree
        plans={plans}
        selectedPlanId={undefined}
        showClosed={false}
        onOpenPlan={vi.fn()}
      />,
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Collapse all" }));
    await user.type(screen.getByRole("searchbox"), "Depth 8");
    expect(screen.getAllByRole("treeitem")).toHaveLength(9);
    expect(
      screen.queryByRole("treeitem", { name: "Other delivery" }),
    ).toBeNull();
    await user.clear(screen.getByRole("searchbox"));
    expect(screen.getAllByRole("treeitem")).toHaveLength(2);
    await user.type(screen.getByRole("searchbox"), "absent");
    expect(screen.getByText("No matching plans.")).toBeDefined();
  });

  it("moves keyboard focus through visible nodes and opens only the chosen plan", async () => {
    const onOpen = vi.fn();
    render(
      <PlanTree
        plans={hierarchy()}
        selectedPlanId="plan-0"
        showClosed={false}
        onOpenPlan={onOpen}
      />,
    );
    const user = userEvent.setup();
    act(() => screen.getByRole("treeitem", { name: "Depth 0" }).focus());
    await user.keyboard("{ArrowRight}{ArrowDown}");
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Depth 2");
    await user.keyboard("{End}{Enter}");
    expect(onOpen.mock.calls).toEqual([["plan-8"]]);
    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Depth 7");
    await user.keyboard("{ArrowLeft}");
    expect(screen.queryByRole("treeitem", { name: "Depth 8" })).toBeNull();
    await user.keyboard("{ArrowRight}{Home}{ArrowLeft}");
    expect(screen.getAllByRole("treeitem")).toHaveLength(1);
    expect(screen.getByRole("treeitem").tabIndex).toBe(0);
  });
});
