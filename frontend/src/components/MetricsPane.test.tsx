import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { MetricsPane } from "./MetricsPane";
import type { MetricsResponse } from "../api/types";
import { jsonResponse } from "../test/appState";

function fixture(): MetricsResponse {
  return {
    generated_at: "2026-07-24T05:00:00Z",
    usage: {
      all_time: { fresh_tokens: 2_100_000, cached_tokens: 88_000_000, total_tokens: 90_100_000 },
      today: { fresh_tokens: 350_000, cached_tokens: 9_000_000, total_tokens: 9_350_000 },
      last_7d: { fresh_tokens: 1_200_000, cached_tokens: 40_000_000, total_tokens: 41_200_000 },
      per_repo: [
        {
          repo: "sulion",
          all_time: { fresh_tokens: 1_500_000, cached_tokens: 60_000_000, total_tokens: 61_500_000 },
          today: { fresh_tokens: 300_000, cached_tokens: 8_000_000, total_tokens: 8_300_000 },
          last_7d: { fresh_tokens: 900_000, cached_tokens: 30_000_000, total_tokens: 30_900_000 },
        },
      ],
      daily: [
        { day: "2026-07-23", fresh_tokens: 500_000, cached_tokens: 20_000_000, total_tokens: 20_500_000 },
        { day: "2026-07-24", fresh_tokens: 350_000, cached_tokens: 9_000_000, total_tokens: 9_350_000 },
      ],
    },
    git: [
      {
        repo: "sulion",
        commits_24h: 3,
        commits_7d: 18,
        insertions_24h: 250,
        deletions_24h: 90,
        insertions_7d: 4_200,
        deletions_7d: 1_300,
        agent_commits_7d: 14,
        human_commits_7d: 4,
        last_commit_at: "2026-07-24T04:00:00Z",
        daily: [
          { day: "2026-07-23", commits: 5, insertions: 900, deletions: 200 },
          { day: "2026-07-24", commits: 3, insertions: 250, deletions: 90 },
        ],
      },
    ],
    churn: [
      {
        repo: "sulion",
        path: "frontend/src/components/MonitorPane.tsx",
        write_turns: 9,
        sessions: 3,
        last_write_at: "2026-07-24T04:30:00Z",
      },
    ],
    flow: {
      wip: 2,
      blocked: 1,
      throughput_weeks: [{ week_start: "2026-07-20", completed_weight: 6 }],
      cycle_time_hours_p50: 5.5,
      cfd: [
        { day: "2026-07-23", pending: 4, in_progress: 2, blocked: 1, completed: 3, skipped: 0 },
        { day: "2026-07-24", pending: 3, in_progress: 2, blocked: 1, completed: 4, skipped: 1 },
      ],
      burndowns: [
        {
          plan_id: "plan-1",
          repo: "sulion",
          title: "Monitor metrics",
          total_weight: 8,
          days: [
            { day: "2026-07-23", remaining_weight: 6, total_weight: 8 },
            { day: "2026-07-24", remaining_weight: 4, total_weight: 8 },
          ],
        },
      ],
    },
  };
}

describe("MetricsPane", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("renders token rollups, flow, git, and churn sections", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonResponse(fixture())),
    );
    render(<MetricsPane />);

    const tokens = await screen.findByRole("region", { name: "Token spend" });
    expect(within(tokens).getByText("350K")).toBeDefined();
    expect(within(tokens).getByText("9.4M processed")).toBeDefined();
    expect(within(tokens).getByText("2.1M")).toBeDefined();

    const flow = screen.getByRole("region", { name: "Delivery flow" });
    expect(within(flow).getByText("2")).toBeDefined();
    expect(within(flow).getByText(/burndown · sulion \/ monitor metrics/i)).toBeDefined();
    expect(within(flow).getByText("4/8 left")).toBeDefined();

    const git = screen.getByRole("region", { name: "Git activity" });
    expect(within(git).getAllByText("18").length).toBeGreaterThan(0);
    expect(within(git).getByText("14 agent · 4 human")).toBeDefined();

    const churn = screen.getByRole("region", { name: "Churn hotspots" });
    expect(
      within(churn).getByText(/MonitorPane\.tsx/),
    ).toBeDefined();
  });

  it("shows day detail in the chart hover tooltip", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => jsonResponse(fixture())),
    );
    render(<MetricsPane />);
    const tokens = await screen.findByRole("region", { name: "Token spend" });
    const bars = within(tokens).getByRole("img", {
      name: "Fresh tokens per day",
    });

    const user = userEvent.setup();
    await user.hover(bars.firstElementChild as HTMLElement);
    await waitFor(() =>
      expect(
        screen.getByText(/500K fresh · 20M cache reads · 20.5M processed/),
      ).toBeDefined(),
    );
  });

  it("surfaces fetch errors", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("nope", { status: 500 })),
    );
    render(<MetricsPane />);
    await waitFor(() => {
      expect(screen.getByText(/metrics unavailable/i)).toBeDefined();
    });
  });
});
