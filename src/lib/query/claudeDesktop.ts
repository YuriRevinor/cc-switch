import { useQuery } from "@tanstack/react-query";
import { claudeDesktopAccountsApi } from "@/lib/api/claudeDesktopAccounts";

/** 各 Claude 账号的 Desktop 数据目录（登的是谁、在不在跑） */
export const claudeDesktopProfilesKey = ["claudeDesktopProfiles"] as const;

/** 和 Desktop 配置检查（`claudeDesktopStatus`）同一节奏 */
const PROFILES_POLL_MS = 5000;

export function useClaudeDesktopProfiles(enabled = true) {
  return useQuery({
    queryKey: claudeDesktopProfilesKey,
    queryFn: () => claudeDesktopAccountsApi.getProfiles(),
    enabled,
    refetchOnWindowFocus: true,
    refetchInterval: PROFILES_POLL_MS,
  });
}

/** 会话同步开关和回收站 */
export const claudeDesktopSessionSyncKey = [
  "claudeDesktopSessionSync",
] as const;

/** 打开 Desktop 时后台同步，回收站可能多出东西：隔一会儿看一次 */
const SESSION_SYNC_POLL_MS = 10000;

export function useClaudeDesktopSessionSync(enabled = true) {
  return useQuery({
    queryKey: claudeDesktopSessionSyncKey,
    queryFn: () => claudeDesktopAccountsApi.getSessionSync(),
    enabled,
    refetchOnWindowFocus: true,
    refetchInterval: SESSION_SYNC_POLL_MS,
  });
}
