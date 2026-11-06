import {
  type ChangeEvent,
  type KeyboardEvent,
  type MouseEvent,
  type FocusEvent,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
} from "react";

import type { PlanSummaryView, PlanView } from "../api/types";
import { Icon } from "../icons";
import type { Maybe } from "../lib/types";
import "./PlanTree.css";

type NavigablePlan = PlanView | PlanSummaryView;

interface TreeNode {
  plan: NavigablePlan;
  children: TreeNode[];
}

interface PlanTreeProps {
  plans: NavigablePlan[];
  selectedPlanId?: string;
  showClosed?: boolean;
  variant?: "browser" | "sidebar";
  label?: string;
  onOpenPlan: (id: string) => void;
}

export function PlanTree({
  plans,
  selectedPlanId,
  showClosed = false,
  variant = "browser",
  label = "Plans",
  onOpenPlan,
}: PlanTreeProps) {
  const [query, setQuery] = useState("");
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [focusedId, setFocusedId] = useState<Maybe<string>>(selectedPlanId);
  const treeRef = useRef<HTMLDivElement>(null);
  const revealedId = useRef<Maybe<string>>(undefined);
  const search = query.trim().toLocaleLowerCase();
  const byId = useMemo(
    () => new Map(plans.map((plan) => [plan.id, plan])),
    [plans],
  );

  // Reveal a newly selected path without discarding other branch choices.
  useEffect(() => {
    if (!selectedPlanId) return;
    setCollapsed((previous) => {
      const next = new Set(previous);
      let parentId = selectedPlanId
        ? byId.get(selectedPlanId)?.parent_plan_id
        : null;
      while (parentId) {
        next.delete(parentId);
        parentId = byId.get(parentId)?.parent_plan_id;
      }
      return next;
    });
    setFocusedId(selectedPlanId);
  }, [byId, selectedPlanId]);

  const roots = useMemo(() => {
    const nodes = new Map<string, TreeNode>();
    for (const plan of plans) {
      const matchesStatus =
        showClosed ||
        plan.status === "active" ||
        plan.status === "paused" ||
        plan.id === selectedPlanId;
      if (
        !matchesStatus ||
        (search && !plan.title.toLocaleLowerCase().includes(search))
      )
        continue;
      // Retain every ancestor so filtering never promotes a branch to a root.
      let current: Maybe<NavigablePlan> = plan;
      while (current && !nodes.has(current.id)) {
        nodes.set(current.id, { plan: current, children: [] });
        current = current.parent_plan_id
          ? byId.get(current.parent_plan_id)
          : undefined;
      }
    }
    const result: TreeNode[] = [];
    for (const plan of plans) {
      const node = nodes.get(plan.id);
      if (!node) continue;
      const parent = plan.parent_plan_id
        ? nodes.get(plan.parent_plan_id)
        : null;
      if (parent) parent.children.push(node);
      else result.push(node);
    }
    return result;
  }, [byId, plans, search, selectedPlanId, showClosed]);

  const visible = useMemo(() => {
    const rows: TreeNode[] = [];
    const visit = (node: TreeNode) => {
      rows.push(node);
      if (search || !collapsed.has(node.plan.id)) node.children.forEach(visit);
    };
    roots.forEach(visit);
    return rows;
  }, [collapsed, roots, search]);
  const tabStop = visible.some((node) => node.plan.id === focusedId)
    ? focusedId
    : visible[0]?.plan.id;

  useEffect(() => {
    if (revealedId.current === selectedPlanId) return;
    const selected = treeRef.current?.querySelector('[aria-selected="true"]');
    if (selected) {
      selected.firstElementChild?.scrollIntoView?.({ block: "nearest" });
      revealedId.current = selectedPlanId;
    }
  }, [selectedPlanId, visible]);

  const toggle = useCallback((id: string) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, []);
  const expandAll = useCallback(() => setCollapsed(new Set()), []);
  const collapseAll = useCallback(
    () => setCollapsed(new Set(plans.map((plan) => plan.id))),
    [plans],
  );
  const onQueryChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => setQuery(event.target.value),
    [],
  );
  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      const id = event.currentTarget.dataset.planId;
      const index = visible.findIndex((node) => node.plan.id === id);
      const node = visible[index];
      if (!node) return;
      let target: Maybe<string>;
      switch (event.key) {
        case "ArrowDown":
          target = visible[index + 1]?.plan.id;
          break;
        case "ArrowUp":
          target = visible[index - 1]?.plan.id;
          break;
        case "Home":
          target = visible[0]?.plan.id;
          break;
        case "End":
          target = visible.at(-1)?.plan.id;
          break;
        case "ArrowRight":
          if (node.children.length && collapsed.has(node.plan.id) && !search)
            toggle(node.plan.id);
          else target = node.children[0]?.plan.id;
          break;
        case "ArrowLeft":
          if (node.children.length && !collapsed.has(node.plan.id) && !search)
            toggle(node.plan.id);
          else target = node.plan.parent_plan_id ?? undefined;
          break;
        case "Enter":
        case " ":
          onOpenPlan(node.plan.id);
          break;
        default:
          return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (target) {
        setFocusedId(target);
        const row = Array.from(
          treeRef.current?.querySelectorAll<HTMLElement>('[role="treeitem"]') ??
            [],
        ).find((element) => element.dataset.planId === target);
        row?.focus();
      }
    },
    [collapsed, onOpenPlan, search, toggle, visible],
  );

  return (
    <>
      {variant === "browser" ? (
        <>
          <input
            type="search"
            aria-label="Search plans"
            placeholder="Find a plan…"
            value={query}
            onChange={onQueryChange}
          />
          <div className="plan-tree__controls">
            <button
              type="button"
              onClick={expandAll}
              disabled={Boolean(search) || plans.length === 0}
            >
              Expand all
            </button>
            <button
              type="button"
              onClick={collapseAll}
              disabled={Boolean(search) || plans.length === 0}
            >
              Collapse all
            </button>
          </div>
        </>
      ) : null}
      <div
        ref={treeRef}
        role="tree"
        aria-label={label}
        className={`plan-tree plan-tree--${variant}`}
      >
        {roots.map((node, index) => (
          <PlanTreeRow
            key={node.plan.id}
            node={node}
            level={1}
            position={index + 1}
            siblingCount={roots.length}
            collapsed={collapsed}
            searching={Boolean(search)}
            selectedPlanId={selectedPlanId}
            tabStop={tabStop}
            onToggle={toggle}
            onOpen={onOpenPlan}
            onKeyDown={onKeyDown}
            onFocus={setFocusedId}
          />
        ))}
      </div>
      {plans.length > 0 && roots.length === 0 ? (
        <p className="plan-tree__empty">
          {search
            ? "No matching plans."
            : "No open plans. Turn on Show closed to browse history."}
        </p>
      ) : null}
    </>
  );
}

interface PlanTreeRowProps {
  node: TreeNode;
  level: number;
  position: number;
  siblingCount: number;
  collapsed: Set<string>;
  searching: boolean;
  selectedPlanId: Maybe<string>;
  tabStop: Maybe<string>;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
  onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
  onFocus: (id: string) => void;
}

function PlanTreeRow({
  node,
  level,
  position,
  siblingCount,
  collapsed,
  searching,
  selectedPlanId,
  tabStop,
  onToggle,
  onOpen,
  onKeyDown,
  onFocus,
}: PlanTreeRowProps) {
  const { plan, children } = node;
  const metadataId = useId();
  const expanded = searching || !collapsed.has(plan.id);
  const open = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      event.stopPropagation();
      onOpen(plan.id);
    },
    [onOpen, plan.id],
  );
  const focus = useCallback(
    (event: FocusEvent<HTMLDivElement>) => {
      event.stopPropagation();
      onFocus(plan.id);
    },
    [onFocus, plan.id],
  );
  const toggle = useCallback(
    (event: MouseEvent<HTMLButtonElement>) => {
      event.stopPropagation();
      // Pointer disclosure also returns keyboard focus to the owning tree item.
      event.currentTarget.parentElement?.parentElement?.focus();
      onToggle(plan.id);
    },
    [onToggle, plan.id],
  );
  const { completed, total, blocked, currentPhase } = planProgress(plan);

  return (
    <div
      role="treeitem"
      aria-label={plan.title}
      aria-describedby={metadataId}
      aria-level={level}
      aria-posinset={position}
      aria-setsize={siblingCount}
      aria-selected={plan.id === selectedPlanId}
      aria-expanded={children.length ? expanded : undefined}
      tabIndex={plan.id === tabStop ? 0 : -1}
      data-plan-id={plan.id}
      className="plan-tree__node"
      onClick={open}
      onKeyDown={onKeyDown}
      onFocus={focus}
    >
      <div className="plan-tree__row">
        {children.length ? (
          <button
            type="button"
            tabIndex={-1}
            className="plan-tree__disclosure"
            onClick={toggle}
            aria-label={`${expanded ? "Collapse" : "Expand"} ${plan.title}`}
            disabled={searching}
          >
            <Icon
              name={expanded ? "chevron-down" : "chevron-right"}
              size={14}
            />
          </button>
        ) : (
          <span className="plan-tree__leaf">
            <Icon name="list-checks" size={12} />
          </span>
        )}
        <span className="plan-tree__body">
          <strong>{plan.title}</strong>
          <span id={metadataId} className="plan-tree__meta">
            <span
              className={`plan-tree__status plan-tree__status--${plan.status}`}
            >
              {plan.status}
            </span>
            <span>
              {completed}/{total} phases
            </span>
            {blocked ? (
              <span className="plan-tree__blocked">{blocked} blocked</span>
            ) : null}
          </span>
          <progress
            className="plan-tree__progress"
            value={completed}
            max={Math.max(total, 1)}
            aria-label={`Phase progress for ${plan.title}`}
          />
          {currentPhase ? (
            <span className="plan-tree__phase">{currentPhase}</span>
          ) : null}
        </span>
      </div>
      {children.length && expanded ? (
        <div role="group" className="plan-tree__children">
          {children.map((child, index) => (
            <PlanTreeRow
              key={child.plan.id}
              node={child}
              level={level + 1}
              position={index + 1}
              siblingCount={children.length}
              collapsed={collapsed}
              searching={searching}
              selectedPlanId={selectedPlanId}
              tabStop={tabStop}
              onToggle={onToggle}
              onOpen={onOpen}
              onKeyDown={onKeyDown}
              onFocus={onFocus}
            />
          ))}
        </div>
      ) : null}
    </div>
  );
}

function planProgress(plan: NavigablePlan) {
  if ("phases" in plan) {
    return {
      completed: plan.phases.filter(
        (phase) => phase.status === "completed" || phase.status === "skipped",
      ).length,
      total: plan.phases.length,
      blocked: plan.phases.filter((phase) => phase.status === "blocked").length,
      currentPhase: null,
    };
  }
  return {
    completed: plan.completed_phases,
    total: plan.total_phases,
    blocked: plan.blocked_phases,
    currentPhase: plan.current_phase_title,
  };
}
