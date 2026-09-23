import type { PlanGuidance } from "../api/types";

export const EMPTY_GUIDANCE: PlanGuidance = {
  outcome: "",
  principles: [],
  assumptions: [],
};

export function cleanGuidance(value: PlanGuidance): PlanGuidance {
  return {
    outcome: value.outcome.trim(),
    principles: value.principles.map((item) => item.trim()).filter(Boolean),
    assumptions: value.assumptions.map((item) => item.trim()).filter(Boolean),
  };
}
