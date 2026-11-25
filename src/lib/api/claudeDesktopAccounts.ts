import { invoke } from "@tauri-apps/api/core";
import { resolveManagedAccountId } from "@/lib/authBinding";
import type { Provider } from "@/types";

/** 某个 Claude 账号的 Desktop 数据目录 */
export interface DesktopProfile {
  /** 目录是给哪个账号建的 */
  accountUuid: string;
  /** 目录里实际登的账号；还没登录时为空 */
  signedInAs?: string | null;
  running: boolean;
}

export interface DesktopProfilesStatus {
  /** 装了能按账号目录打开的 Claude Desktop（macOS，或 Windows 的 MSIX 安装版）：Desktop 官方卡才能绑 Claude 账号 */
  supported: boolean;
  /** 默认数据目录登的号在授权中心里的 key（原来那张官方卡按它显示账号和额度） */
  defaultAccountKey?: string | null;
  defaultRunning: boolean;
  profiles: DesktopProfile[];
}

export const claudeDesktopAccountsApi = {
  async getProfiles(): Promise<DesktopProfilesStatus> {
    return await invoke("get_claude_desktop_profiles");
  },

  /**
   * 打开卡片对应的 Desktop：`accountKey` 是账号卡绑的 Claude 账号，`null` 用默认数据目录。别的 Desktop
   * 开着时要 `quitRunning`（用户确认过）才会先退出它们。返回是不是第一次用这个账号目录（要登录）。
   */
  async open(accountKey: string | null, quitRunning = false): Promise<boolean> {
    return await invoke("open_claude_desktop", { accountKey, quitRunning });
  },
};

/** 原来那张固定 id 的官方卡：代表 Desktop 自己的登录，不能绑 Claude 账号 */
export const CLAUDE_DESKTOP_OFFICIAL_ID = "claude-desktop-official";

/** Desktop 的账号卡：绑了 Claude 账号的官方卡（不含原来那张），返回账号 key */
export function desktopAccountKey(
  provider: Pick<Provider, "id" | "category" | "meta">,
): string | null {
  if (
    provider.category !== "official" ||
    provider.id === CLAUDE_DESKTOP_OFFICIAL_ID
  ) {
    return null;
  }
  return (
    resolveManagedAccountId(provider.meta, "claude_official")?.trim() || null
  );
}

/**
 * 添加 Desktop 官方预设时：没选 Claude 账号就是原来那张固定 id 的官方卡（只有一张，已有就不再加）；
 * 选了账号是一张新的账号卡，照常新建。
 */
export function addsDesktopDefaultOfficial(
  isOfficialPreset: boolean,
  meta: Provider["meta"],
): boolean {
  return (
    isOfficialPreset &&
    !resolveManagedAccountId(meta, "claude_official")?.trim()
  );
}

/** 账号卡对应的 Desktop：账号目录，或默认数据目录登的正好是这个号时的默认目录 */
function accountDesktop(status: DesktopProfilesStatus, accountKey: string) {
  const uuid = accountKey.split(":")[0];
  const profile = status.profiles.find((p) => p.accountUuid === uuid);
  if (profile) return profile;
  const isDefault = Boolean(status.defaultAccountKey?.startsWith(`${uuid}:`));
  return isDefault
    ? { signedInAs: uuid, running: status.defaultRunning }
    : null;
}

/** 卡片对应的 Desktop 开着没有：账号卡看它的数据目录，别的卡看默认数据目录 */
export function desktopRunning(
  status: DesktopProfilesStatus,
  accountKey: string | null,
): boolean {
  if (accountKey === null) return status.defaultRunning;
  return accountDesktop(status, accountKey)?.running ?? false;
}

/** 切到这张账号卡要先在 Desktop 里登录（还没有登好这个号的目录） */
export function desktopNeedsLogin(
  status: DesktopProfilesStatus,
  accountKey: string,
): boolean {
  return !accountDesktop(status, accountKey)?.signedInAs;
}
