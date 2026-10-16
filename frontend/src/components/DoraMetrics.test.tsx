import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import {
  getDoraDeployments,
  getDoraMetrics,
  updateDeploymentOutcome,
} from "../api/client";
import type { DoraMetricsView } from "../api/types";
import { DoraMetrics } from "./DoraMetrics";

vi.mock("../api/client", () => ({
  getDoraDeployments: vi.fn(),
  getDoraMetrics: vi.fn(),
  updateDeploymentOutcome: vi.fn(),
}));

function fixture(): DoraMetricsView {
  const data: DoraMetricsView = {
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
  data.summary = {
    ...data.repos[0],
    repo: "",
    repository_count: 1,
    collected_repository_count: 1,
    recent_deployments: [],
  };
  return data;
}

describe("DoraMetrics", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(getDoraDeployments).mockResolvedValue({
      deployments: fixture().repos[0].recent_deployments,
      next_cursor: null,
    });
    vi.mocked(getDoraMetrics).mockResolvedValue(fixture());
  });

  it("shows all five DORA measurements with their classification coverage", () => {
    render(<DoraMetrics data={fixture()} />);
    const table = screen.getByRole("region", { name: "DORA metrics" });
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
    data.summary = {
      ...data.repos[0],
      repo: "",
      repository_count: 1,
      collected_repository_count: 1,
    };
    render(<DoraMetrics data={data} />);
    const table = screen.getByRole("region", { name: "DORA metrics" });
    expect(within(table).getByText("0.00/day")).toBeDefined();
    expect(within(table).getAllByText("—")).toHaveLength(4);
    expect(within(table).getByText("1/2 outcomes")).toBeDefined();
    expect(within(table).getByText("1 recovered · 1 open")).toBeDefined();
  });

  it("records an explicit production outcome and refreshes the rollup", async () => {
    const user = userEvent.setup();
    const data = fixture();
    vi.mocked(updateDeploymentOutcome).mockResolvedValue(
      data.repos[0].recent_deployments[0],
    );
    render(<DoraMetrics data={data} />);
    await user.click(screen.getByText(/Deployment details/));
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
    expect(getDoraMetrics).toHaveBeenCalledOnce();
    await waitFor(() => expect(screen.queryByRole("form")).toBeNull());
  });

  it("preserves the editable draft and shows a rejected outcome", async () => {
    const user = userEvent.setup();
    vi.mocked(updateDeploymentOutcome).mockRejectedValue(
      new Error("Failure time must be between deployment and now."),
    );
    render(<DoraMetrics data={fixture()} />);
    await user.click(screen.getByText(/Deployment details/));
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
    expect(getDoraMetrics).not.toHaveBeenCalled();
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
    vi.mocked(getDoraDeployments).mockResolvedValue({
      deployments: [deployment],
      next_cursor: null,
    });
    render(<DoraMetrics data={data} />);
    await user.click(screen.getByText(/Deployment details/));
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
    render(<DoraMetrics />);
    expect(
      screen.getByText("Aggregate deployment metrics are not available yet."),
    ).toBeDefined();
  });

  it("does not substitute repository measurements for a missing aggregate", () => {
    const data = fixture();
    delete data.summary;
    const { container } = render(<DoraMetrics data={data} />);
    expect(container.querySelectorAll(".metrics-tile")).toHaveLength(0);
    expect(screen.queryByText("1.5h")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(getDoraDeployments).not.toHaveBeenCalled();
  });

  it("retains an outcome refresh when parent metrics arrive while it is pending", async () => {
    const user = userEvent.setup();
    const data = fixture();
    let resolveRefresh!: (value: DoraMetricsView) => void;
    vi.mocked(getDoraMetrics).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveRefresh = resolve;
        }),
    );
    vi.mocked(updateDeploymentOutcome).mockResolvedValue({
      ...data.repos[0].recent_deployments[0],
      outcome: "healthy",
    });
    const { rerender } = render(<DoraMetrics data={data} />);
    await user.click(screen.getByText(/Deployment details/));
    await user.click(
      await screen.findByRole("button", { name: "Record outcome" }),
    );
    await user.selectOptions(
      screen.getByLabelText("Production outcome"),
      "healthy",
    );
    await user.click(screen.getByRole("button", { name: "Save outcome" }));
    await waitFor(() => expect(getDoraMetrics).toHaveBeenCalledOnce());
    rerender(<DoraMetrics data={fixture()} />);
    const refreshed = fixture();
    refreshed.summary!.deployments = 3;
    resolveRefresh(refreshed);
    expect(await screen.findByText(/3 deployments/)).toBeDefined();
    await waitFor(() => expect(screen.queryByRole("form")).toBeNull());
  });

  it("retries a failed lazy history read without losing the summary", async () => {
    const user = userEvent.setup();
    vi.mocked(getDoraDeployments).mockRejectedValueOnce(
      new Error("History unavailable"),
    );
    render(<DoraMetrics data={fixture()} />);
    await user.click(screen.getByText(/Deployment details/));
    expect((await screen.findByRole("alert")).textContent).toContain(
      "History unavailable",
    );
    expect(screen.getByText("0.07/day")).toBeDefined();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(
      await screen.findByRole("button", { name: "Record outcome" }),
    ).toBeDefined();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("shows only portfolio aggregates across fifty repositories", async () => {
    const data = fixture();
    data.repos = Array.from({ length: 50 }, (_, index) => ({
      ...data.repos[0],
      repo: `repo-${index + 1}`,
      change_lead_time_hours_p50: index + 1,
    }));
    data.summary = {
      ...data.summary!,
      deployments: 100,
      deployments_per_day: 100 / 30,
      repository_count: 50,
      collected_repository_count: 50,
    };
    const { container } = render(<DoraMetrics data={data} />);
    expect(container.querySelectorAll(".metrics-tile")).toHaveLength(5);
    expect(container.querySelectorAll("details")).toHaveLength(1);
    expect(screen.queryByRole("table")).toBeNull();
    expect(screen.queryByRole("combobox")).toBeNull();
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("1.5h")).toBeDefined();
    expect(screen.queryByText("2.1d")).toBeNull();
    expect(screen.getByText("3.33/day")).toBeDefined();
    expect(
      screen.getByText(/100 deployments.*50\/50 repositories/),
    ).toBeDefined();
    expect(getDoraDeployments).not.toHaveBeenCalled();
    const user = userEvent.setup();
    await user.click(screen.getByText(/Deployment details/));
    await waitFor(() =>
      expect(getDoraDeployments).toHaveBeenCalledWith(
        undefined,
        undefined,
        expect.any(AbortSignal),
      ),
    );
  });

  it("loads deployment history in pages only when requested", async () => {
    const user = userEvent.setup();
    const deployment = fixture().repos[0].recent_deployments[0];
    const cursor = { deployed_at: deployment.deployed_at, id: deployment.id };
    vi.mocked(getDoraDeployments)
      .mockResolvedValueOnce({ deployments: [deployment], next_cursor: cursor })
      .mockResolvedValueOnce({
        deployments: [
          { ...deployment, id: "deployment-2", revision: "12345678abcd" },
        ],
        next_cursor: null,
      });
    render(<DoraMetrics data={fixture()} />);
    expect(getDoraDeployments).not.toHaveBeenCalled();
    await user.click(screen.getByText(/Deployment details/));
    await user.click(
      await screen.findByRole("button", { name: "Show more deployments" }),
    );
    expect(await screen.findByRole("link", { name: "12345678" })).toBeDefined();
    expect(screen.getByRole("link", { name: "abcdef12" })).toBeDefined();
    expect(getDoraDeployments).toHaveBeenLastCalledWith(
      undefined,
      cursor,
      expect.any(AbortSignal),
    );
    expect(
      screen.queryByRole("button", { name: "Show more deployments" }),
    ).toBeNull();
  });

  it("aborts deployment history when the metrics screen becomes inactive", async () => {
    const user = userEvent.setup();
    const data = fixture();
    let resolveOld!: (
      page: Awaited<ReturnType<typeof getDoraDeployments>>,
    ) => void;
    vi.mocked(getDoraDeployments).mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOld = resolve;
        }),
    );
    const { rerender } = render(<DoraMetrics data={data} />);
    await user.click(screen.getByText(/Deployment details/));
    await waitFor(() => expect(getDoraDeployments).toHaveBeenCalledOnce());
    const signal = vi.mocked(getDoraDeployments).mock.calls[0][2]!;
    rerender(<DoraMetrics data={data} active={false} />);
    expect(signal.aborted).toBe(true);
    resolveOld({
      deployments: data.repos[0].recent_deployments,
      next_cursor: null,
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: "Record outcome" }),
      ).toBeNull(),
    );
  });
});
