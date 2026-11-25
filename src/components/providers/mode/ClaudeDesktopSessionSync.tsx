import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { toast } from "@/lib/toast";
import { claudeDesktopAccountsApi } from "@/lib/api/claudeDesktopAccounts";
import {
  claudeDesktopSessionSyncKey,
  useClaudeDesktopProfiles,
  useClaudeDesktopSessionSync,
} from "@/lib/query/claudeDesktop";
import { extractErrorMessage } from "@/utils/errorUtils";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { ConfirmDialog } from "@/components/ConfirmDialog";

/**
 * Claude Desktop 页上的「会话同步」：多账号同步删除和归档 Code 会话的开关，和「最近删除」（CC Switch
 * 的回收站，见后端 `claude_desktop_sync` / `claude_desktop_trash`）。同步发生在打开 Desktop 之前，回收站
 * 多出会话时提示一次。
 */
export function ClaudeDesktopSessionSync() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const { data: profiles } = useClaudeDesktopProfiles();
  const supported = Boolean(profiles?.supported);
  const { data } = useClaudeDesktopSessionSync(supported);
  const [trashOpen, setTrashOpen] = useState(false);
  const [confirmEmpty, setConfirmEmpty] = useState(false);
  const seenUpTo = useRef<number | null>(null);

  const trash = data?.trash ?? [];
  const latest = trash.reduce(
    (max, entry) => Math.max(max, entry.trashedAt),
    0,
  );

  // 第一次拿到数据只记下来；之后回收站多了新挪进来的会话就提示
  useEffect(() => {
    if (!data) return;
    if (seenUpTo.current === null) {
      seenUpTo.current = latest;
      return;
    }
    const since = seenUpTo.current;
    if (latest > since) {
      const count = data.trash.filter(
        (entry) => entry.trashedAt > since,
      ).length;
      toast.info(
        t("claudeDesktop.sessionSync.synced", {
          count,
          defaultValue:
            "已在其他账号同步删除 {{count}} 个会话，可在「最近删除」里恢复",
        }),
      );
    }
    seenUpTo.current = Math.max(since, latest);
  }, [data, latest, t]);

  const refresh = () =>
    queryClient.invalidateQueries({ queryKey: claudeDesktopSessionSyncKey });

  const toggle = useMutation({
    mutationFn: (enabled: boolean) =>
      claudeDesktopAccountsApi.setSessionSync(enabled),
    onSettled: refresh,
    onError: (error) =>
      toast.error(
        t("claudeDesktop.sessionSync.toggleFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
          defaultValue: "设置失败：{{detail}}",
        }),
      ),
  });

  const restore = useMutation({
    mutationFn: (entry: { sessionId: string; title: string }) =>
      claudeDesktopAccountsApi.restoreSession(entry.sessionId),
    onSuccess: (report, entry) => {
      const title =
        entry.title ||
        t("claudeDesktop.sessionSync.untitled", { defaultValue: "（无标题）" });
      if (report.waiting > 0) {
        toast.warning(
          t("claudeDesktop.sessionSync.restoredWaiting", {
            title,
            count: report.waiting,
            defaultValue:
              "已恢复「{{title}}」，还有 {{count}} 份要等对应的 Claude Desktop 退出后再点一次恢复",
          }),
        );
      } else {
        toast.success(
          t("claudeDesktop.sessionSync.restored", {
            title,
            defaultValue:
              "已恢复「{{title}}」，下次打开 Claude Desktop 时补回所有账号",
          }),
        );
      }
    },
    onError: (error) =>
      toast.error(
        t("claudeDesktop.sessionSync.restoreFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
          defaultValue: "恢复失败：{{detail}}",
        }),
      ),
    onSettled: refresh,
  });

  const empty = useMutation({
    mutationFn: () => claudeDesktopAccountsApi.emptySessionTrash(),
    onSuccess: () => {
      setConfirmEmpty(false);
      toast.success(
        t("claudeDesktop.sessionSync.emptied", {
          defaultValue: "回收站已清空",
        }),
      );
    },
    onError: (error) =>
      toast.error(
        t("claudeDesktop.sessionSync.emptyFailed", {
          detail: extractErrorMessage(error) || t("common.unknown"),
          defaultValue: "清空失败：{{detail}}",
        }),
      ),
    onSettled: refresh,
  });

  if (!supported || !data) return null;

  return (
    <div
      className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-panel border border-border bg-surface px-4 py-2.5"
      data-testid="claude-desktop-session-sync"
    >
      <div className="min-w-0 flex-1">
        <div className="text-body font-medium text-fg-1">
          {t("claudeDesktop.sessionSync.title", {
            defaultValue: "多账号同步删除和归档",
          })}
        </div>
        <div className="text-caption text-fg-3">
          {t("claudeDesktop.sessionSync.description", {
            defaultValue:
              "在任一账号里删除或归档 Code 会话，其他账号跟着删除或归档；删掉的先放进回收站，可以恢复。下次通过 CC Switch 打开 Claude Desktop 时生效。",
          })}
        </div>
      </div>
      <Button
        variant="neutral"
        size="compact"
        onClick={() => setTrashOpen(true)}
      >
        {t("claudeDesktop.sessionSync.trashButton", {
          count: trash.length,
          defaultValue: "最近删除（{{count}}）",
        })}
      </Button>
      <Switch
        checked={data.enabled}
        disabled={toggle.isPending}
        onCheckedChange={(enabled) => toggle.mutate(enabled)}
        aria-label={t("claudeDesktop.sessionSync.title", {
          defaultValue: "多账号同步删除和归档",
        })}
      />

      <Dialog open={trashOpen} onOpenChange={setTrashOpen}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>
              {t("claudeDesktop.sessionSync.trashTitle", {
                defaultValue: "最近删除的会话",
              })}
            </DialogTitle>
            <DialogDescription>
              {t("claudeDesktop.sessionSync.trashDescription", {
                defaultValue:
                  "在某个账号里删除后，CC Switch 把其他账号里的副本（以及不再用到的对话记录、工作区）挪到了这里。恢复后会重新出现在所有账号里；对应的 Claude Desktop 开着时，要等它退出后再恢复。",
              })}
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 overflow-y-auto px-6">
            {trash.length === 0 ? (
              <div className="py-6 text-center text-body text-fg-3">
                {t("claudeDesktop.sessionSync.trashEmpty", {
                  defaultValue: "回收站是空的",
                })}
              </div>
            ) : (
              <ul className="divide-y divide-border">
                {trash.map((entry) => (
                  <li
                    key={entry.sessionId}
                    className="flex items-center gap-3 py-2"
                  >
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-body text-fg-1">
                        {entry.title ||
                          t("claudeDesktop.sessionSync.untitled", {
                            defaultValue: "（无标题）",
                          })}
                      </div>
                      <div className="text-caption text-fg-3">
                        {new Date(entry.trashedAt).toLocaleString()} ·{" "}
                        {t("claudeDesktop.sessionSync.copies", {
                          count: entry.copies,
                          defaultValue: "{{count}} 份副本",
                        })}
                        {entry.transcripts > 0 &&
                          ` · ${t("claudeDesktop.sessionSync.withTranscripts", {
                            defaultValue: "含对话记录",
                          })}`}
                      </div>
                    </div>
                    <Button
                      variant="neutral"
                      size="compact"
                      disabled={restore.isPending}
                      onClick={() =>
                        restore.mutate({
                          sessionId: entry.sessionId,
                          title: entry.title,
                        })
                      }
                    >
                      {t("claudeDesktop.sessionSync.restore", {
                        defaultValue: "恢复",
                      })}
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <DialogFooter>
            <Button
              variant="neutral"
              size="compact"
              disabled={trash.length === 0 || empty.isPending}
              onClick={() => setConfirmEmpty(true)}
            >
              {t("claudeDesktop.sessionSync.emptyTrash", {
                defaultValue: "清空回收站",
              })}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        isOpen={confirmEmpty}
        variant="destructive"
        zIndex="alert"
        title={t("claudeDesktop.sessionSync.emptyTrash", {
          defaultValue: "清空回收站",
        })}
        message={t("claudeDesktop.sessionSync.emptyConfirmMessage", {
          defaultValue:
            "回收站里的会话、对话记录和工作区会被永久删除，不能恢复。",
        })}
        confirmText={t("claudeDesktop.sessionSync.emptyTrash", {
          defaultValue: "清空回收站",
        })}
        pending={empty.isPending}
        onConfirm={() => empty.mutate()}
        onCancel={() => setConfirmEmpty(false)}
      />
    </div>
  );
}
