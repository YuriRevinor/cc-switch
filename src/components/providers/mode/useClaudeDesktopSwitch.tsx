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
  /** 账号卡绑的号；`null` 是默认数据目录的卡（原来那张官方卡、第三方） */
  accountKey: string | null;
  /** 账号的邮箱，认不出时用卡名 */
  login: string;
  firstLogin: boolean;
  /** 别的 Desktop 开着，要先退出 */
  quitRunning: boolean;
  /** 还没切：确认后先切换再打开。没有就是只打开 */
  doSwitch?: DesktopDoSwitch;
}

/**
 * Claude Desktop 页的切换（见后端 `claude_desktop_account`）。账号卡和别的卡一样「切换」，切过去后
 * 把 Desktop 换成这张卡的：账号卡用这个号的数据目录打开，别的卡用默认数据目录。别的 Desktop 开着、
 * 或第一次要登录时先确认，取消就什么都不变。默认数据目录的卡、又没有账号目录的 Desktop 开着时照旧
 * 只改配置，不主动开 Desktop。
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
      claudeDesktopAccountsApi.open(pending.accountKey, pending.quitRunning),
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

  /**
   * 要不要动 Desktop；`null` 是照旧（只切换）。现查一次各 Desktop 在不在跑：轮询的结果可能是几秒前的。
   */
  const plan = async (
    provider: Provider,
    launch: boolean,
  ): Promise<Pending | null> => {
    const status = await queryClient
      .fetchQuery({
        queryKey: claudeDesktopProfilesKey,
        queryFn: () => claudeDesktopAccountsApi.getProfiles(),
        staleTime: 0,
      })
      .catch(() => profiles);
    if (!status?.supported) return null;
    const accountKey = desktopAccountKey(provider);
    const running = desktopRunning(status, accountKey);
    const anyRunning =
      status.defaultRunning || status.profiles.some((p) => p.running);
    const quitRunning = !running && anyRunning;
    if (accountKey === null && !quitRunning && !launch) return null;
    return {
      provider,
      accountKey,
      login:
        accounts?.accounts.find((account) => account.id === accountKey)
          ?.login ?? provider.name,
      firstLogin:
        accountKey !== null &&
        !running &&
        desktopNeedsLogin(status, accountKey),
      quitRunning,
    };
  };

  const run = async (pending: Pending) => {
    if (pending.doSwitch && !(await pending.doSwitch(true))) return;
    open.mutate(pending);
  };

  const start = (pending: Pending | null) => {
    if (!pending) return;
    if (pending.firstLogin || pending.quitRunning) setConfirm(pending);
    else void run(pending);
  };

  const dialog = (
    <ConfirmDialog
      isOpen={confirm !== null}
      variant="info"
      title={t("claudeDesktop.launch.confirmTitle", {
        name: confirm?.provider.name ?? "",
        defaultValue: "用「{{name}}」打开 Claude Desktop",
      })}
      message={[
        confirm?.quitRunning &&
          t("claudeDesktop.launch.quitMessage", {
            name: confirm.provider.name,
            defaultValue:
              "会先退出正在运行的 Claude Desktop（进行中的对话和 Code 会话会中断），再打开「{{name}}」的 Claude Desktop。",
          }),
        confirm?.firstLogin &&
          t("claudeDesktop.launch.firstLoginMessage", {
            login: confirm.login,
            defaultValue:
              "这是第一次用这个账号打开：在打开的 Desktop 里登录 {{login}}，登录保存在这个账号单独的数据目录里，以后切到这张卡就是这个号。",
          }),
      ]
        .filter(Boolean)
        .join("\n\n")}
      confirmText={
        confirm?.quitRunning
          ? t("claudeDesktop.launch.quitConfirm", {
              defaultValue: "退出并打开",
            })
          : t("claudeDesktop.launch.firstLoginConfirm", {
              defaultValue: "打开并登录",
            })
      }
      pending={open.isPending}
      onConfirm={() => {
        if (confirm) void run(confirm);
        setConfirm(null);
      }}
      onCancel={() => setConfirm(null)}
    />
  );

  return {
    /**
     * 直连下切到这张卡：要换 Desktop 时先确认（取消就不切），切过去后把 Desktop 换成这张卡的；
     * 不用换 Desktop 时直接 `doSwitch(false)`，和别的应用一样。
     */
    switchTo: (provider: Provider, doSwitch: DesktopDoSwitch) =>
      void plan(provider, false).then((pending) => {
        if (pending) start({ ...pending, doSwitch });
        else void doSwitch(false);
      }),
    /** 已经切过去了（回到直连并改用这家）：把 Desktop 换成这张卡的 */
    afterSwitch: (provider: Provider) => void plan(provider, false).then(start),
    /** 使用中的账号卡上「打开 Claude Desktop」：从程序坞打开的是默认数据目录，不是这个号 */
    canOpen: (provider: Provider) =>
      enabled && supported && desktopAccountKey(provider) !== null,
    open: (provider: Provider) => void plan(provider, true).then(start),
    dialog,
  };
}
