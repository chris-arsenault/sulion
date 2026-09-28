import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AuthGate, AuthProvider } from "./AuthProvider";
import { revokeBrowserSessions } from "../api/client";
import { signOut } from "./cognito";

vi.mock("./cognito", () => ({
  isAuthConfigured: () => true,
  getSessionSnapshot: async () => ({ accessToken: "fixture", username: "test", email: null }),
  signOut: vi.fn(), signIn: vi.fn(),
}));
vi.mock("../api/client", () => ({ revokeBrowserSessions: vi.fn() }));

describe("server-backed sign-out", () => {
  beforeEach(() => { vi.clearAllMocks(); });
  it("clears local login only after durable revocation succeeds", async () => {
    let finish!: () => void;
    vi.mocked(revokeBrowserSessions).mockReturnValue(new Promise<void>((resolve) => { finish = resolve; }));
    render(<AuthProvider><AuthGate><div>session</div></AuthGate></AuthProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "sign out all Sulion sessions" }));
    expect(signOut).not.toHaveBeenCalled();
    finish();
    await waitFor(() => expect(signOut).toHaveBeenCalledOnce());
  });
  it("reports failed revocation and retains the login so it can be retried", async () => {
    vi.mocked(revokeBrowserSessions).mockRejectedValue(new Error("broker unavailable"));
    render(<AuthProvider><AuthGate><div>session</div></AuthGate></AuthProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "sign out all Sulion sessions" }));
    expect((await screen.findByRole("alert")).textContent).toContain("broker unavailable");
    expect(signOut).not.toHaveBeenCalled();
  });
});
