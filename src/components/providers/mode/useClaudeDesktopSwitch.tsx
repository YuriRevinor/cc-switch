import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "@/lib/toast";
import { authApi } from "@/lib/api";
import {
  claudeDesktopAccountsApi,
  desktopAccountKey,
  desktopNeedsLogin,
  desktopRunning,
} from "@/lib/api/claudeDesktopAccounts";
import {
  claudeDesktopProfilesKey,
  useClaudeDesktopProfiles,
} from "@/lib/query/claudeDesktop";
import type { Provider } from "@/types";
import { extractErrorMessage } from "@/utils/errorUtils";
import { ConfirmDialog } from "@/components/ConfirmDialog";

/** 切换（`quiet`：成功提示由这里出）；返回切过去没有 */
export type DesktopDoSwitch = (quiet: boolean) => Promise<boolean>;

interface Pending {
  provider: Provider;
  /** 账号卡绑的号 */
  accountKey: string;
  /** 账号的邮箱，认不出时用卡名 */
  login: string;
  firstLogin: boolean;
}

/**
 * Claude Desktop 页的切换（见后端 `claude_desktop_account`）。切换只改配置，从不退出、也不启动
 * Desktop；只有用户在使用中的账号卡上点「打开 Claude Desktop」才用这个号的数据目录启动它。别的
 * Desktop 开着时后端不打开，提示用户自己先退出；第一次要登录时先确认。
 */
export function useClaudeDesktopSwitch(enabled: boolean) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: profiles } = useClaudeDesktopProfiles(enabled);
  const supported = Boolean(profiles?.supported);
  const { data: accounts } = useQuery({
    queryKey: ["managed-auth-status", "claude_official"],
    queryFn: () => authApi.authGetStatus("claude_official"),
    staleTime: 30000,
    enabled: enabled && supported,
  });
  const [confirm, setConfirm] = useState<Pending | null>(null);

  const open = useMutation({
    mutationFn: (pending: Pending) =>
      claudeDesktopAccountsApi.open(pending.accountKey),
    onSuccess: (firstLogin, pending) =>
      toast.success(
        firstLogin
          ? t("claudeDesktop.launch.openedForLogin", {
              login: pending.login,
              defaultValue:
                "已打开新的 Claude Desktop 窗口，请在里面登录 {{login}}",
            })
          : t("claudeDesktop.launch.opened", {
              name: pending.provider.name,
              defaultValue: "已用「{{name}}」打开 Claude Desktop",
            }),
        { closeButton: firstLogin },
      ),
    onError: (error) =>
      toast.error(
        t("claudeDesktop.launch.failed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
          defaultValue: "打开 Claude Desktop 失败：{{detail}}",
        }),
      ),
    onSettled: () =>
      void queryClient.invalidateQueries({
        queryKey: claudeDesktopProfilesKey,
      }),
  });

  /** 打开前现查一次各 Desktop 在不在跑：轮询的结果可能是几秒前的。 */
  const plan = async (provider: Provider): Promise<Pending | null> => {
    const status = await queryClient
      .fetchQuery({
        queryKey: claudeDesktopProfilesKey,
        queryFn: () => claudeDesktopAccountsApi.getProfiles(),
        staleTime: 0,
      })
      .catch(() => profiles);
    if (!status?.supported) return null;
    const accountKey = desktopAccountKey(provider);
    if (accountKey === null) return null;
    const running = desktopRunning(status, accountKey);
    return {
      provider,
      accountKey,
      login:
        accounts?.accounts.find((account) => account.id === accountKey)
          ?.login ?? provider.name,
      firstLogin: !running && desktopNeedsLogin(status, accountKey),
    };
  };

  const start = (pending: Pending | null) => {
    if (!pending) return;
    if (pending.firstLogin) setConfirm(pending);
    else open.mutate(pending);
  };

  const dialog = (
    <ConfirmDialog
      isOpen={confirm !== null}
      variant="info"
      title={t("claudeDesktop.launch.confirmTitle", {
        name: confirm?.provider.name ?? "",
        defaultValue: "用「{{name}}」打开 Claude Desktop",
      })}
      message={t("claudeDesktop.launch.firstLoginMessage", {
        login: confirm?.login ?? "",
        defaultValue:
          "这是第一次用这个账号打开：在打开的 Desktop 里登录 {{login}}，登录保存在这个账号单独的数据目录里，以后切到这张卡就是这个号。",
      })}
      confirmText={t("claudeDesktop.launch.firstLoginConfirm", {
        defaultValue: "打开并登录",
      })}
      pending={open.isPending}
      onConfirm={() => {
        if (confirm) open.mutate(confirm);
        setConfirm(null);
      }}
      onCancel={() => setConfirm(null)}
    />
  );

  return {
    /** 直连下切到这张卡：只改配置，和别的应用一样，不碰正在运行的 Desktop */
    switchTo: (_provider: Provider, doSwitch: DesktopDoSwitch) =>
      void doSwitch(false),
    /** 使用中的账号卡上「打开 Claude Desktop」：从开始菜单打开的是默认数据目录，不是这个号 */
    canOpen: (provider: Provider) =>
      enabled && supported && desktopAccountKey(provider) !== null,
    open: (provider: Provider) => void plan(provider).then(start),
    dialog,
  };
}
