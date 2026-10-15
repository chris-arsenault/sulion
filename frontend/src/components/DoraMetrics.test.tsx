import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { updateDeploymentOutcome } from "../api/client";
import type { DoraMetricsView } from "../api/types";
import { DoraMetrics } from "./DoraMetrics";

vi.mock("../api/client", () => ({ updateDeploymentOutcome: vi.fn() }));

function fixture(): DoraMetricsView {
  return {
    window_days: 30,
    repos: [
      {
        repo: "sulion",
        checked_at: "2026-10-14T12:00:00Z",
        scanned_since: "2026-09-14T12:00:00Z",
        collection_error: null,
        deployments: 2,
        deployments_per_day: 2 / 30,
        change_lead_time_hours_p50: 1.5,
        lead_time_samples: 3,
        classified_outcomes: 2,
        failed_deployments: 1,
        change_failure_rate: 0.5,
        recovery_time_hours_p50: 0.25,
        recovered_deployments: 1,
        open_failures: 0,
        classified_rework: 2,
        rework_rate: 0,
        recent_deployments: [
          {
            id: "deployment-1",
            repo_name: "sulion",
            revision: "abcdef1234567890",
            deployed_at: "2026-10-13T12:00:00Z",
            source_url: "https://github.com/acme/app/actions/runs/1",
            outcome: "unknown",
            failed_at: null,
            recovered_at: null,
            rework: null,
          },
        ],
      },
    ],
  };
}

describe("DoraMetrics", () => {
  beforeEach(() => vi.resetAllMocks());

  it("shows all five DORA measurements with their classification coverage", () => {
    render(<DoraMetrics data={fixture()} onChanged={vi.fn()} />);
    const table = screen.getByRole("table", { name: "DORA by repository" });
    expect(within(table).getByText("0.07/day")).toBeDefined();
    expect(within(table).getByText("1.5h")).toBeDefined();
    expect(within(table).getByText("50.0%")).toBeDefined();
    expect(within(table).getByText("15m")).toBeDefined();
    expect(within(table).getByText("0.0%")).toBeDefined();
    expect(within(table).getByText("2/2 outcomes")).toBeDefined();
    expect(within(table).getByText("3 measured commits")).toBeDefined();
  });

  it("keeps missing evidence distinct from measured zero", () => {
    const data = fixture();
    Object.assign(data.repos[0], {
      deployments_per_day: 0,
      change_lead_time_hours_p50: null,
      change_failure_rate: null,
      recovery_time_hours_p50: null,
      rework_rate: null,
      classified_outcomes: 1,
      classified_rework: 0,
      open_failures: 1,
    });
    render(<DoraMetrics data={data} onChanged={vi.fn()} />);
    const table = screen.getByRole("table");
    expect(within(table).getByText("0.00/day")).toBeDefined();
    expect(within(table).getAllByText("—")).toHaveLength(4);
    expect(within(table).getByText("1/2 outcomes")).toBeDefined();
    expect(within(table).getByText("1 recovered · 1 open")).toBeDefined();
  });

  it("records an explicit production outcome and refreshes the rollup", async () => {
    const user = userEvent.setup();
    const data = fixture();
    const onChanged = vi.fn().mockResolvedValue(undefined);
    vi.mocked(updateDeploymentOutcome).mockResolvedValue(
      data.repos[0].recent_deployments[0],
    );
    render(<DoraMetrics data={data} onChanged={onChanged} />);
    await user.click(screen.getByText("sulion · deployment details"));
    await user.click(
      await screen.findByRole("button", { name: "Record outcome" }),
    );
    await user.selectOptions(
      screen.getByLabelText("Production outcome"),
      "healthy",
    );
    await user.selectOptions(
      screen.getByLabelText("Deployment purpose"),
      "planned",
    );
    await user.click(screen.getByRole("button", { name: "Save outcome" }));
    await waitFor(() =>
      expect(updateDeploymentOutcome).toHaveBeenCalledWith("deployment-1", {
        outcome: "healthy",
        failed_at: null,
        recovered_at: null,
        rework: false,
      }),
    );
    expect(onChanged).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByRole("form")).toBeNull());
  });

  it("preserves the editable draft and shows a rejected outcome", async () => {
    const user = userEvent.setup();
    const onChanged = vi.fn();
    vi.mocked(updateDeploymentOutcome).mockRejectedValue(
      new Error("Failure time must be between deployment and now."),
    );
    render(<DoraMetrics data={fixture()} onChanged={onChanged} />);
    await user.click(screen.getByText("sulion · deployment details"));
    await user.click(
      await screen.findByRole("button", { name: "Record outcome" }),
    );
    await user.selectOptions(
      screen.getByLabelText("Production outcome"),
      "healthy",
    );
    await user.click(screen.getByRole("button", { name: "Save outcome" }));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "Failure time",
    );
    expect(
      (screen.getByLabelText("Production outcome") as HTMLSelectElement).value,
    ).toBe("healthy");
    expect(onChanged).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Save outcome" })).toBeDefined();
  });

  it("preserves failure and recovery timestamps when saving existing evidence", async () => {
    const user = userEvent.setup();
    const data = fixture();
    const deployment = data.repos[0].recent_deployments[0];
    Object.assign(deployment, {
      outcome: "failed",
      failed_at: "2026-10-13T12:05:23.123Z",
      recovered_at: "2026-10-13T12:35:42.456Z",
      rework: true,
    });
    vi.mocked(updateDeploymentOutcome).mockResolvedValue(deployment);
    render(
      <DoraMetrics
        data={data}
        onChanged={vi.fn().mockResolvedValue(undefined)}
      />,
    );
    await user.click(screen.getByText("sulion · deployment details"));
    await user.click(
      await screen.findByRole("button", { name: "Record outcome" }),
    );
    await user.click(screen.getByRole("button", { name: "Save outcome" }));
    await waitFor(() =>
      expect(updateDeploymentOutcome).toHaveBeenCalledWith(deployment.id, {
        outcome: "failed",
        failed_at: deployment.failed_at,
        recovered_at: deployment.recovered_at,
        rework: true,
      }),
    );
  });

  it("supports an older control response without DORA data", () => {
    render(<DoraMetrics onChanged={vi.fn()} />);
    expect(
      screen.getByText("No production deployment history collected yet."),
    ).toBeDefined();
  });
});
