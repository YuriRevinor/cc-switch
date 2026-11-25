import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ClaudeDesktopSessionSync } from "@/components/providers/mode/ClaudeDesktopSessionSync";
import type { SessionSyncStatus } from "@/lib/api/claudeDesktopAccounts";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  getProfiles: vi.fn(),
  getSessionSync: vi.fn(),
  setSessionSync: vi.fn(),
  restoreSession: vi.fn(),
  emptySessionTrash: vi.fn(),
}));

vi.mock("@/lib/api/claudeDesktopAccounts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/claudeDesktopAccounts")>()),
  claudeDesktopAccountsApi: mocks,
}));

const SESSION = "00000000-0000-4000-8000-000000000101";
const status = (enabled: boolean): SessionSyncStatus => ({
  enabled,
  trash: [
    {
      sessionId: SESSION,
      title: "修 bug",
      trashedAt: 1_795_600_000_000,
      copies: 2,
      transcripts: 1,
    },
  ],
});

describe("ClaudeDesktopSessionSync", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    mocks.getProfiles.mockResolvedValue({
      supported: true,
      defaultAccountKey: null,
      defaultRunning: false,
      profiles: [],
    });
    mocks.getSessionSync.mockResolvedValue(status(true));
    mocks.setSessionSync.mockResolvedValue(undefined);
    mocks.restoreSession.mockResolvedValue({ restored: 2, waiting: 0 });
    mocks.emptySessionTrash.mockResolvedValue(undefined);
  });

  it("turns the sync off from the switch", async () => {
    renderWithQueryClient(<ClaudeDesktopSessionSync />);
    const toggle = await screen.findByRole("switch");
    expect(toggle).toHaveAttribute("data-state", "checked");

    await userEvent.click(toggle);
    await waitFor(() =>
      expect(mocks.setSessionSync).toHaveBeenCalledWith(false),
    );
  });

  it("restores a session from the trash", async () => {
    renderWithQueryClient(<ClaudeDesktopSessionSync />);
    await userEvent.click(
      await screen.findByRole("button", { name: "最近删除（1）" }),
    );
    expect(await screen.findByText("修 bug")).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "恢复" }));
    await waitFor(() =>
      expect(mocks.restoreSession).toHaveBeenCalledWith(SESSION),
    );
  });

  it("empties the trash only after confirming", async () => {
    renderWithQueryClient(<ClaudeDesktopSessionSync />);
    await userEvent.click(
      await screen.findByRole("button", { name: "最近删除（1）" }),
    );
    await userEvent.click(screen.getByRole("button", { name: "清空回收站" }));
    expect(mocks.emptySessionTrash).not.toHaveBeenCalled();

    const buttons = await screen.findAllByRole("button", {
      name: "清空回收站",
    });
    await userEvent.click(buttons[buttons.length - 1]);
    await waitFor(() => expect(mocks.emptySessionTrash).toHaveBeenCalled());
  });

  it("shows nothing when Desktop accounts are not supported", async () => {
    mocks.getProfiles.mockResolvedValue({
      supported: false,
      defaultAccountKey: null,
      defaultRunning: false,
      profiles: [],
    });
    renderWithQueryClient(<ClaudeDesktopSessionSync />);
    await waitFor(() => expect(mocks.getProfiles).toHaveBeenCalled());
    expect(screen.queryByTestId("claude-desktop-session-sync")).toBeNull();
    expect(mocks.getSessionSync).not.toHaveBeenCalled();
  });
});
