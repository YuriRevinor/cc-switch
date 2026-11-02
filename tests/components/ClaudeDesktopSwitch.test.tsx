import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useClaudeDesktopSwitch } from "@/components/providers/mode/useClaudeDesktopSwitch";
import type { DesktopProfilesStatus } from "@/lib/api/claudeDesktopAccounts";
import type { Provider } from "@/types";
import { renderWithQueryClient } from "../utils/testQueryClient";

const mocks = vi.hoisted(() => ({
  doSwitch: vi.fn(),
  getProfiles: vi.fn(),
  open: vi.fn(),
}));

vi.mock("@/lib/api/claudeDesktopAccounts", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api/claudeDesktopAccounts")>()),
  claudeDesktopAccountsApi: {
    getProfiles: mocks.getProfiles,
    open: mocks.open,
  },
}));

const ACCOUNT = "aaaaaaaa-0000-4000-8000-000000000001";
const card: Provider = {
  id: "alt",
  name: "小号",
  category: "official",
  settingsConfig: {},
  meta: {
    authBinding: {
      source: "managed_account",
      authProvider: "claude_official",
      accountId: `${ACCOUNT}:org`,
    },
  },
};

const profiles = (defaultRunning: boolean): DesktopProfilesStatus => ({
  supported: true,
  defaultAccountKey: null,
  defaultRunning,
  profiles: [{ accountUuid: ACCOUNT, signedInAs: ACCOUNT, running: false }],
});

function Harness() {
  const desktop = useClaudeDesktopSwitch(true);
  return (
    <>
      <button
        type="button"
        onClick={() => desktop.switchTo(card, mocks.doSwitch)}
      >
        switch
      </button>
      {desktop.dialog}
    </>
  );
}

describe("useClaudeDesktopSwitch", () => {
  beforeEach(() => {
    Object.values(mocks).forEach((mock) => mock.mockReset());
    mocks.doSwitch.mockResolvedValue(true);
    mocks.open.mockResolvedValue(false);
  });

  it("switches to an account card, then opens that account's Desktop", async () => {
    mocks.getProfiles.mockResolvedValue(profiles(false));
    renderWithQueryClient(<Harness />);
    await userEvent.click(screen.getByRole("button", { name: "switch" }));

    await waitFor(() =>
      expect(mocks.open).toHaveBeenCalledWith(`${ACCOUNT}:org`, false),
    );
    expect(mocks.doSwitch).toHaveBeenCalledWith(true);
  });

  it("quits another running Desktop only after the user confirms", async () => {
    mocks.getProfiles.mockResolvedValue(profiles(true));
    renderWithQueryClient(<Harness />);

    await userEvent.click(screen.getByRole("button", { name: "switch" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "common.cancel" }),
    );
    expect(mocks.doSwitch).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: "switch" }));
    await userEvent.click(
      await screen.findByRole("button", { name: "退出并打开" }),
    );
    await waitFor(() =>
      expect(mocks.open).toHaveBeenCalledWith(`${ACCOUNT}:org`, true),
    );
  });

  it("leaves Desktop alone when the switch itself fails", async () => {
    mocks.getProfiles.mockResolvedValue(profiles(false));
    mocks.doSwitch.mockResolvedValue(false);
    renderWithQueryClient(<Harness />);
    await userEvent.click(screen.getByRole("button", { name: "switch" }));

    await waitFor(() => expect(mocks.doSwitch).toHaveBeenCalled());
    expect(mocks.open).not.toHaveBeenCalled();
  });
});
