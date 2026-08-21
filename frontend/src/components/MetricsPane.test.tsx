import { afterEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { MetricsPane } from "./MetricsPane";
import type { JobsResponse, MetricsResponse } from "../api/types";
import { jsonResponse } from "../test/appState";

function jobsFixture(): JobsResponse {
  return {
    active: [
      {
        id: 3,
        name: "transcript_catchup_claude-code",
        label: "claude-code transcript catch-up",
        status: "running",
        progress_current: 72,
        progress_total: 176,
        unit: "files",
        detail: "/projects/x/subagents/agent-a1.jsonl",
        error: null,
        started_at: "2026-07-24T04:40:00Z",
        updated_at: "2026-07-24T04:59:00Z",
        finished_at: null,
        stalled: false,
      },
    ],
    recent: [
      {
        id: 2,
        name: "timeline_backfill",
        label: "Timeline projection rebuild",
        status: "completed",
        progress_current: 420,
        progress_total: 420,
        unit: "sessions",
        detail: null,
        error: null,
        started_at: "2026-07-24T04:00:00Z",
        updated_at: "2026-07-24T04:12:00Z",
        finished_at: "2026-07-24T04:12:00Z",
        stalled: false,
      },
    ],
  };
}

function stubFetch() {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) =>
      String(input).includes("/api/jobs")
        ? jsonResponse(jobsFixture())
        : jsonResponse(fixture()),
    ),
  );
}

function fixture(): MetricsResponse {
  return {
    generated_at: "2026-07-24T05:00:00Z",
    usage: {
      all_time: { input_tokens: 1_600_000, cache_write_input_tokens: 400_000, cached_input_tokens: 88_000_000, output_tokens: 500_000, estimated_cost_usd: 67.5, unpriced_tokens: 0 },
      today: { input_tokens: 300_000, cache_write_input_tokens: 50_000, cached_input_tokens: 9_000_000, output_tokens: 50_000, estimated_cost_usd: 7.5, unpriced_tokens: 0 },
      last_7d: { input_tokens: 900_000, cache_write_input_tokens: 200_000, cached_input_tokens: 40_000_000, output_tokens: 300_000, estimated_cost_usd: 34.75, unpriced_tokens: 0 },
      per_repo: [
        {
          repo: "sulion",
          all_time: { input_tokens: 1_100_000, cache_write_input_tokens: 300_000, cached_input_tokens: 60_000_000, output_tokens: 400_000, estimated_cost_usd: 48, unpriced_tokens: 0 },
          today: { input_tokens: 250_000, cache_write_input_tokens: 40_000, cached_input_tokens: 8_000_000, output_tokens: 50_000, estimated_cost_usd: 6.5, unpriced_tokens: 0 },
          last_7d: { input_tokens: 650_000, cache_write_input_tokens: 150_000, cached_input_tokens: 30_000_000, output_tokens: 250_000, estimated_cost_usd: 27, unpriced_tokens: 0 },
        },
      ],
      by_model: [
        {
          model: "claude-opus-5",
          agent: "claude-code",
          usage: { input_tokens: 900_000, cache_write_input_tokens: 200_000, cached_input_tokens: 40_000_000, output_tokens: 300_000, estimated_cost_usd: 34.75, unpriced_tokens: 0 },
          price: { input_usd_per_million: 5, cached_input_usd_per_million: 0.5, cache_write_usd_per_million: 6.25, cache_write_1h_usd_per_million: 10, output_usd_per_million: 25 },
        },
      ],
      model_window_days: 14,
      daily: [
        { day: "2026-07-23", input_tokens: 500_000, cache_write_input_tokens: 100_000, cached_input_tokens: 20_000_000, output_tokens: 100_000, estimated_cost_usd: 16.5, unpriced_tokens: 0 },
        { day: "2026-07-24", input_tokens: 300_000, cache_write_input_tokens: 50_000, cached_input_tokens: 9_000_000, output_tokens: 50_000, estimated_cost_usd: 7.5, unpriced_tokens: 0 },
      ],
      pricing: {
        basis: "API list price (standard tier)",
        as_of: "2026-08-19",
        openai_source_url: "https://developers.openai.com/api/docs/models/gpt-5.6-sol",
        anthropic_source_url: "https://claude.com/pricing",
        note: "Subscription fees and long-context multipliers are not included.",
      },
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
    stubFetch();
    render(<MetricsPane />);

    const tokens = await screen.findByRole("region", { name: "Token spend" });
    expect(within(tokens).getByText("~$7.50")).toBeDefined();
    expect(within(tokens).getByText("claude-opus-5")).toBeDefined();
    expect(within(tokens).getAllByText("300K").length).toBeGreaterThan(0);
    expect(within(tokens).queryByText(/processed/i)).toBeNull();

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

  it("shows active background jobs with progress and recent history", async () => {
    stubFetch();
    render(<MetricsPane />);

    const jobs = await screen.findByRole("region", { name: "Background jobs" });
    expect(
      within(jobs).getByText("claude-code transcript catch-up"),
    ).toBeDefined();
    expect(within(jobs).getByText(/72\/176 files/)).toBeDefined();
    expect(within(jobs).getByText(/agent-a1\.jsonl/)).toBeDefined();
    expect(within(jobs).getByText("Timeline projection rebuild")).toBeDefined();
    expect(within(jobs).getByText("completed")).toBeDefined();
  });

  it("shows day detail in the chart hover tooltip", async () => {
    stubFetch();
    render(<MetricsPane />);
    const tokens = await screen.findByRole("region", { name: "Token spend" });
    const bars = within(tokens).getByRole("img", {
      name: "Estimated API cost per day",
    });

    const user = userEvent.setup();
    await user.hover(bars.firstElementChild as HTMLElement);
    await waitFor(() =>
      expect(
        screen.getByText(/500K input \(100K cache writes\).*20M cached input.*100K output/s),
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
