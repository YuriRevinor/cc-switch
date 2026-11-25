# Claude Desktop 多账号（含 Windows 移植）：本地改动与合并说明

本文记录分支 `merge-dey410-claude-desktop-accounts` 相对上游主线的全部改动，供以后合并上游
（`origin` = https://github.com/farion1231/cc-switch ）时对照。分支只在本地，没有推送。

## 1. 基点和提交

- 上游基点：`99f4bae5`（2026-11-25，`fix(agy): prevent cached Antigravity auth status …`，v4.0.41 之后）。
- 来源 fork：`dey410` = https://github.com/Dey410/cc-switch ，提交 `8770712de0bfa7b51c618131bd374725d37d9ca2`
  （作者 Xiaozhu_410），只支持 macOS。

| 提交 | 内容 | 来源 |
|---|---|---|
| `d70f706d` | feat(claude-desktop): switch between Claude accounts with per-account data folders | cherry-pick 自 `8770712d`，手工解决冲突 |
| `d2d7b9a9` | feat(claude-desktop): per-account Desktop data folders on Windows (MSIX) | 本地：Windows 移植 |
| `f7a70673` | fix(claude-desktop): don't re-add deleted sessions or ones whose transcript is gone | 本地 |
| `5fd1eea4` | fix(claude-desktop): never quit or auto-start Claude Desktop; keep 1P sign-in | 本地 |
| `f4c167e4` | feat(claude-desktop): --open-claude-desktop command-line flag for hotkeys | 本地 |
| `7460123a` | Claude Desktop: scratch sessions copied across data dirs show as 此电脑 (junction + transcript hardlinks) | 本地 |
| `942f9fd9` | feat(claude-desktop): cc-switch-open.exe hotkey launcher; activate a running Desktop directly | 本地 |

合计 42 个文件，约 +2600 / −150 行（`git diff --stat 99f4bae5..HEAD`）。

**合并上游的推荐做法**：在新的上游主线上按上表顺序逐个 `git cherry-pick`（用 `git log --oneline 99f4bae5..HEAD`
取完整列表），每一步解决冲突、跑第 6 节的检查。如果上游已经合入了 Dey410 的提交（搜 `claude_desktop_account.rs`
是否存在），就跳过 `d70f706d`，把后面的提交在上游版本上重新套用。

## 2. 功能概览（最终行为）

1. **账号卡**：Claude Desktop 页的官方卡可以绑定授权中心（`claude_official`）里的 Claude 账号。原来那张固定
   id 的官方卡 `claude-desktop-official` 不能绑，代表默认数据目录里 Desktop 自己的登录。
2. **每个账号一个 Desktop 数据目录**：通过 Electron 的 `--user-data-dir` 启动。这不是 Anthropic 公开支持的用法，
   以后的 Desktop 版本可能失效。
   - macOS：`~/Library/Application Support/com.ccswitch.desktop/claude-desktop-accounts/<accountUuid>/`
   - Windows：`%USERPROFILE%\.cc-switch-claude-desktop-accounts\<accountUuid>\`
3. **切换只改配置**（`5fd1eea4`）：切到任何卡都只写 Desktop 的配置文件（账号卡是官方卡，写回 1P），
   **不结束、也不启动** Claude Desktop。
4. **手动打开**：使用中的账号卡上有「打开 Claude Desktop」按钮，用这个号的数据目录启动。别的 Desktop 开着时
   **不会替用户关闭它**，只报错提示用户自己从托盘退出。目标目录已经开着时只叫到前面。
5. **命令行 `--open-claude-desktop`**（`f4c167e4`）：效果等于在 Claude Desktop 页当前使用中的卡上点「打开」。
   给 PowerToys 等快捷键工具用，见第 5 节。
6. **Code 会话补齐**：Desktop「Code」标签页的会话列表是按账号分开存的，对话记录却和账号无关。打开 Desktop 前，
   把各账号文件夹缺的会话列表文件从别处复制过去。只补缺，不覆盖、不删除（`f7a70673` 又加了两条不补的规则，见 4.3）。
7. **「此电脑」会话**（`7460123a`）：没选文件夹的会话复制到别的数据目录后仍显示为「此电脑」，
   靠目录链接 + 对话记录硬链接实现，见 4.5。
8. **快捷键启动器 `cc-switch-open.exe`**：比 `cc-switch.exe --open-claude-desktop` 快，见第 5 节。

## 3. 文件清单（按提交）

### 3.1 `d70f706d`（Dey410 原提交，macOS）

后端（Rust，`src-tauri/src/`）：
- **新增** `services/claude_desktop_account.rs`：账号目录、进程检测、打开/叫到前面、状态查询。
- **新增** `services/claude_desktop_sessions.rs`：会话补齐。
- **新增** `commands/claude_desktop_account.rs`：`get_claude_desktop_profiles`、`open_claude_desktop` 两个命令。
- 改动：
  - `commands/mod.rs`、`services/mod.rs`：模块注册；
  - `lib.rs`：在 `invoke_handler` 里注册上面两个命令；
  - `claude_desktop_config.rs`：`is_official_provider` 认账号卡，新增 `desktop_data_dir` / `macos_app_bundle`；
  - `commands/provider.rs`、`services/provider/mod.rs`：切换和校验能认出账号卡；
  - `services/claude_account/snapshot/mod.rs`；
  - `tray.rs`：托盘按账号显示额度，切到账号卡时交给界面。

前端（`src/`）：
- **新增**：
  - `lib/api/claudeDesktopAccounts.ts`、`lib/query/claudeDesktop.ts`；
  - `components/providers/mode/useClaudeDesktopSwitch.tsx`。
- 改动：
  - `App.tsx`、`hooks/useTrayNavigation.ts`、`hooks/useProviderActions.ts`；
  - `components/providers/{AddProviderDialog,ProviderCard,presentation}.tsx/ts`；
  - `forms/{ClaudeDesktopProviderForm,ClaudeOfficialSection,shared/ManagedAccountSelect}.tsx`；
  - `mode/SwitchModePanel.tsx`。
- i18n：8 个语言文件（de/en/ja/ko/ru/tr/zh/zh-TW）。
- 测试：`tests/components/` 下 6 个文件、`tests/hooks/useProviderActions.test.tsx`。
- 文档：`docs/guides/claude-official-accounts-guide-zh.md`。

**当初 cherry-pick 的冲突**（约 15 个文件，以下几处最难，下次大概率还会冲突）：
- `src/i18n/locales/*.json`：上游频繁加键，冲突在同一段落。
  - 用 `tests/config/localeParity.test.ts` 检查 8 个语言的键是否一致。de/ko/ru/tr 当时缺的翻译是手工补的。
- `ClaudeOfficialSection.tsx`：自动合并把帮助文案拼坏了，最后整段按 fork 的版本重写。
- `SwitchModePanel.tsx`、`ProviderCard.tsx`、`tray.rs`、`services/provider/mod.rs`：上游改动多的大文件，
  要逐块看。
- `tests/components/ClaudeDesktopProviderForm.test.tsx`。

### 3.2 `d2d7b9a9`（Windows 移植）

- `src-tauri/Cargo.toml`：`windows-sys` 增加 features `Wdk_System_Threading`、`Win32_System_Diagnostics_ToolHelp`。
  合并时注意：上游如果改了这一段 feature 列表，要把这两项保留下来。
- `services/claude_desktop_account.rs`：
  - `supported()`、`default_dir()`、`accounts_root()`、`same_dir()`、`launch()`、`bring_to_front()`
    按平台分开实现；
  - 新增 `mod win`（只在 Windows 编译）：
    - 用 `CreateToolhelp32Snapshot` 枚举进程；
    - 用 `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` 加 `QueryFullProcessImageNameW` 取 exe 路径；
    - 用 `NtQueryInformationProcess(ProcessCommandLineInformation)` 取命令行，缓冲区按 u64 对齐；
    - 用 `CommandLineToArgvW` 拆参数；
  - 新增 `windows_desktop_main()`：判断一个 `claude.exe` 是不是 Desktop 主进程。
    - exe 路径必须在 `\WindowsApps\Claude_*\app\Claude.exe`；
    - 参数里不能有 `--type=`；
    - 解析 `--user-data-dir=`。
- `claude_desktop_config.rs`：`desktop_data_dir`、`macos_app_bundle` 加 `#[cfg_attr(windows, allow(dead_code))]`。
- `services/provider/mod.rs`：不支持时的报错文案提到 Windows MSIX。
- 前端和 i18n：帮助文案从「macOS」改成「macOS 和 Windows」（8 个语言文件里的 `claudeHelp`，
  以及 `ClaudeOfficialSection.tsx` 的 defaultValue）。
- 文档：`docs/guides/claude-official-accounts-guide-zh.md` 第 5 节改成「macOS / Windows」，加了 Windows 注意事项。

### 3.3 `f7a70673`（会话补齐不再“删了又回来”）

- 只改了 `services/claude_desktop_sessions.rs`。
- 新增 CC Switch 自己的记录文件 `~/.cc-switch/claude-desktop-sessions-seen.json`，
  格式是 `{"version":1,"folders":{"<文件夹路径>":["local_<id>.json",…]}}`。

### 3.4 `5fd1eea4`（不自动关闭/启动 Desktop）

- `services/claude_desktop_account.rs`：
  - 删掉 `quit()`（macOS 发 SIGTERM、Windows `taskkill /T /F`）和 `win::kill_tree`；
  - `open(account_key: &str)` 不再接受 `quit_running`；
  - 别的 Desktop 开着时返回 `claude_desktop.account.others_running` 错误。
- `commands/claude_desktop_account.rs`：`open_claude_desktop(account_key: String)`，去掉 `quit_running` 参数。
- `win::launch(profile_dir, first_party)`：`first_party` 为真时总是带 `--claude-first-party-sign-in`，
  默认数据目录也带。
- 前端：
  - `useClaudeDesktopSwitch.tsx`：`switchTo` 只调用 `doSwitch(false)`；删掉 `afterSwitch` 和「退出并打开」确认框；
    只保留第一次登录的确认；
  - `SwitchModePanel.tsx`：删掉回到直连后自动换 Desktop 的调用；
  - `claudeDesktopAccounts.ts`：`open(accountKey: string)`。
- `tests/components/ClaudeDesktopSwitch.test.tsx`：改成测新行为（切换不碰 Desktop、只在用户点击时打开、
  第一次登录先确认）。
- 遗留：i18n 里 `claudeDesktop.launch.quitMessage` / `quitConfirm` 已经不用了，没删，以免触发语言一致性测试。
  可以以后一起删掉。

### 3.5 `f4c167e4`（命令行 `--open-claude-desktop`）

- `services/claude_desktop_account.rs`：
  - `pub const OPEN_FLAG`；
  - `open_current(state)` 按当前卡决定目标，判断逻辑在纯函数 `open_target()` 里，有单元测试；
  - `open_dir(profile_dir, first_party)` 是 `open` 和 `open_current` 共用的打开流程；
  - `open_from_command_line(app)` 在后台执行，出错时用 `MessageBoxW` 弹系统对话框。
- `lib.rs`：
  - `static OPEN_CLAUDE_DESKTOP_AT_START`，在 `run()` 开头读 `std::env::args()`；
  - `handle_second_instance()` 开头遇到这个参数时，只打开 Desktop 然后 `return`，不显示主窗口、不退出轻量模式；
  - 冷启动时，Windows 的 `on_page_load` 和 `setup` 里窗口显示的分支都按「静默启动」处理，保持隐藏；
  - `app.manage(app_state)` 之后调用 `open_from_command_line`。
- 合并时注意：`lib.rs` 是上游改动最多的文件之一。这几处只是插入代码，冲突了照着上面的位置重新插入即可。

### 3.6 `7460123a`（「此电脑」会话）

- **新增** `services/claude_desktop_scratch.rs`：识别别的数据目录的 scratch 会话、建目录链接、硬链接对话记录、
  改会话文件里的路径；`project_slug()` 复刻 Claude Code 的项目目录命名（含超过 200 字符时的哈希后缀）。
- `services/claude_desktop_sessions.rs`：
  - `Session` 多存一份解析好的 `doc`；
  - `fill_with(dirs, &Places)`：`Places` 里有记录文件、备份目录、还在的对话记录、projects 目录；
  - 补齐前先就地转换以前原样复制过来的 scratch 会话（先备份到 `~/.cc-switch/claude-desktop-sessions-backup/<时间>/<账号>/<组织>/`）；
  - 复制时如果是别的数据目录的 scratch 会话，写入改过路径的版本（`write_if_absent`），否则原样复制。
- `services/mod.rs`：注册模块。

### 3.7 快捷键启动器、直接激活窗口

- **新增** `src-tauri/src/bin/cc-switch-open.rs`（第二个 `[[bin]]`，release 约 280 KB）：
  - 找 single-instance 插件的消息窗口 `com.ccswitch.desktop-sic` / `com.ccswitch.desktop-siw`，
    用 `WM_COPYDATA`（`dwData = 1542`，内容 `"<cwd>|<cc-switch.exe 路径>|--open-claude-desktop\0"`）把参数发过去；
  - 发之前 `AllowSetForegroundWindow(ASFW_ANY)`，让 CC Switch 能把 Desktop 拿到前台；
  - 找不到窗口（CC Switch 没开）就启动同目录的 `cc-switch.exe --open-claude-desktop`。
  - **合并时注意**：协议照抄 `tauri-plugin-single-instance` 2.4.0 的 `platform_impl/windows.rs`。插件升级后要核对
    类名、`dwData` 和分隔符；如果打开了插件的 `semver` feature，窗口类名会带版本号，启动器要跟着改。
    `IDENTIFIER` 要和 `tauri.conf.json` 一致。
- `src-tauri/Cargo.toml`：`[package] default-run = "cc-switch"`；新增 `[[bin]] cc-switch-open`；`windows-sys` 加
  `Win32_System_DataExchange`。
- `services/claude_desktop_account.rs`：
  - `open_dir`：目标 Desktop 已经开着时**先**叫到前面，再补齐别的目录；没开着时照旧先补齐再启动；
  - `win::activate_window(pid)`：枚举顶层窗口，找到这个进程看得见、没有所有者、有标题的窗口，最小化的先还原，
    `SetForegroundWindow`。找不到（缩在托盘里）或拿不到前台时，退回原来的「同目录再启动一次」。

## 4. 关键设计和踩过的坑（换上游版本时最容易出问题的地方）

### 4.1 Claude Desktop（Windows MSIX 版，包名 `Claude_pzs8sxrjxfjjc`）的行为

以下是 2026-11 在 Desktop 6.28899.6 上观察到的，换版本后要复查。

- **启动器**：执行别名 `%LOCALAPPDATA%\Microsoft\WindowsApps\claude-desktop.exe`，是 0 字节的重解析点。
  判断时不要跟随链接取元数据。
- **默认数据目录**：`%LOCALAPPDATA%\Claude-Data\claude_pzs8sxrjxfjjc\`。
- **全局 1P/3P 配置**：`%LOCALAPPDATA%\Claude-3p\claude_desktop_config.json` 的 `deploymentMode`。
  这一份不跟着数据目录走。
- **写虚拟化**：MSIX 包开着文件系统写虚拟化。AppData 下除了 `Claude-3p`、`Claude-Data`、`Claude\logs`，
  写入都会被重定向进包的私有目录，CC Switch 读不到。所以 Windows 的账号目录放在 `%USERPROFILE%` 下。
- **`--user-data-dir` 只在 1P 模式下生效**。
- **未登录时 Desktop 会自己清掉 1P 选择**：
  - 数据目录没有登录 cookie（新建的账号目录，或在默认目录里退出了登录），又不是带
    `--claude-first-party-sign-in` 启动时，Desktop 启动后会在日志里写 `[custom-3p] deploymentMode written { mode: undefined }`，
    `first_party_pick: 'dropped'`。
  - 结果是 `Claude-3p` 里的 `deploymentMode` 被删掉，登录页变成可以选网关（3P）的那种。
  - 这就是用户看到的「登录时变成 gateway」。不是 CC Switch 写的：CC Switch 给官方卡写的是 `1p`。
  - 所以 CC Switch 启动官方卡时总是带这个参数。但用户从开始菜单直接启动 Desktop 时，CC Switch 管不到。
- **关窗口只是缩到托盘**。旧版用 `taskkill /T /F` 退出，`5fd1eea4` 已经删掉，CC Switch 不再结束 Desktop。
- **单实例锁按数据目录**：同一个目录再启动一次，会把已经开着的窗口叫出来。`bring_to_front` 在 Windows 上就是这样实现的。
  不同目录的两个 Desktop 可以同时运行，这会让用户分不清在哪个窗口里退出登录，所以打开前要求别的 Desktop 都已退出。
- **日志**：
  - 默认目录：`%LOCALAPPDATA%\Claude\logs\main.log`；
  - 账号目录：`<账号目录>\logs\main.log`。

### 4.2 Code 会话的存放

- 列表：`<数据目录>\claude-code-sessions\<accountUuid>\<orgUuid>\local_<uuid>.json`。
  关键字段有 `cliSessionId`、`cwd`、`lastActivityAt`、`isArchived`。
- 对话记录：`~/.claude/projects/<项目>/<cliSessionId>.jsonl`，和账号无关。
- Desktop 删会话时：
  - 删列表文件和会话目录；
  - 只有当前数据目录（所有账号）和 Desktop 自己知道的几个固定目录里都没有别的记录引用这个 `cliSessionId` 时，
    才把对话记录移走；
  - CC Switch 的账号目录**不在**它查的范围里。

### 4.3 会话补齐规则（`claude_desktop_sessions.rs`）

- 只补缺，不覆盖、不删除。同一个会话有多份时，取 `lastActivityAt` 最新的那份。
- 正在运行的 Desktop 的目录不写，否则它会用内存里的列表覆盖回去。
- 不补的两种情况：
  1. 对话记录已经不在了。分不清的时候照补：projects 目录读不了、远程会话、没有 `cliSessionId`。
  2. 这个文件夹以前出现过、现在没了，说明是在这个号里删的。依据 `claude-desktop-sessions-seen.json`，
     这份记录只加不减。文件夹整个空了的时候不按记录挡，记录也重新开始。
- 记录文件读坏了，就从空的开始；写不进去，只记一条警告。最坏的结果只是删过的会话再被补回来一次。

### 4.5 「此电脑」会话（`claude_desktop_scratch.rs`）

- Desktop 给没选文件夹的会话建临时工作区 `<数据目录>\scratch-workspaces\<accountUuid>\<orgUuid>\scratch-<日期>-<6 位>`，
  会话的 `originCwd` 指向它。侧栏只在 `originCwd` 落在**当前** `app.getPath('userData')\scratch-workspaces\<a>\<b>\<c>`
  下（任何账号都行）时显示「此电脑」，否则显示文件夹名（例如 `scratch-2026-11-12-71ff96`）。
- 所以把别的数据目录的这种会话放进数据目录 D 时：
  1. 在 `D\scratch-workspaces\<目标文件夹的账号>\<组织>\<同名>` 建目录链接（Windows 用 `cmd /D /C mklink /J`，
     不需要管理员；别处用符号链接），指向原工作区的 canonical 路径（去掉 `\\?\`）。原工作区已经不在了就建空目录；
     那个位置已有别的东西就放弃，照旧原样复制。路径含 `"` 或 `%` 时不调 cmd。
  2. 把 `cliSessionId`、`priorCliSessionIds` 对应的 `<id>.jsonl` 和 `<id>/` 子目录，从旧 cwd 的项目目录硬链接到
     新 cwd 的项目目录（每个 projects 根都做），硬链接失败就复制。已有的不覆盖。
  3. 所有以旧工作区开头的字符串换成新路径（不分大小写、按分隔符断开），名字含 `realpath` 的字段
     （`folderPlace.realPath`、`gitAnchorsFolderRealpath`）不改，因为链接解析后就是原目录。
  4. 同一个数据目录内的复制（只是换账号文件夹）不处理：Desktop 本来就显示「此电脑」。
- **项目目录名**：非 `[A-Za-z0-9]` 的 UTF-16 码元换成 `-`；超过 200 个字符时截到 200，再接
  `-` + 对原路径做 32 位 `h = (h<<5) - h + c`、取 `Math.abs` 后的 36 进制。已用真实目录 `…-scratch-2026-1-j3hcd8` 验证，见单元测试。
- **Desktop 6.28899 的删除行为**（`app.asar` 里 `.vite/build/index.chunk-cpsM356y.js`）：
  - `removeScratchWorkspace`：`fs.rm(工作区, {recursive, force})`。实测 Node 22 对 junction 只删链接本身，不进原目录；
  - 定期清理（`startScratchWorkspaceSweep`）先 `lstat`，不是目录（链接）就跳过；
  - 删会话时移走对话记录前要求 `nlink === 1`，硬链接过的对话记录它不删（会话没了，文件留着）。
- **风险**：在**原件所在的数据目录**里删这个会话，Desktop 会删掉真实工作区，其他目录里的链接就断了，
  工作区里的文件没了（对话记录因为是硬链接，还在）。要彻底避免，得把工作区挪到 CC Switch 自己的目录、
  每个数据目录都只放链接；目前没这样做，因为那要改动原件。
- 转换以前原样复制的会话只在目录空闲（那个 Desktop 没开着）时进行，改之前备份。

### 4.4 不自动关闭/启动的原则

- 切换卡片只写配置。Desktop 只在用户主动操作时启动：点按钮，或执行命令行参数。
- 任何代码路径都不结束 Desktop 进程。以后合并上游或 Dey410 的新版本时，不要把 `quit` / `taskkill` 加回来。

## 5. 命令行参数和 PowerToys 快捷键

`cc-switch.exe --open-claude-desktop` 按 Claude Desktop 页当前使用中的卡决定打开方式：

| 当前卡 | 打开的数据目录 | 带 `--claude-first-party-sign-in` |
|---|---|---|
| 账号卡（绑了 Claude 账号） | 这个号的账号目录；没建过、而默认目录登的正好是这个号时用默认目录 | 是 |
| 原来那张官方卡 `claude-desktop-official`、或还没选过卡 | 默认数据目录 | 是 |
| 第三方 / 网关卡 | 默认数据目录 | 否 |

- 打开前先补齐会话（目标已经开着时先叫到前面，再补齐别的目录）。
- 有别的 Desktop 开着时不打开，弹出系统对话框，请用户先从托盘退出 Claude。
- 目标目录的 Desktop 已经开着时，只把它叫到前面：窗口看得见就直接激活，缩在托盘里才再启动一次让它自己出来。
- CC Switch 已经在运行：参数通过 single-instance 交给正在运行的实例处理，不开第二个窗口，也不显示主窗口。
- CC Switch 没运行：冷启动后主窗口保持隐藏（和静默启动一样），只留托盘，然后执行打开。

**推荐用 `cc-switch-open.exe`**（和 `cc-switch.exe` 放在同一个文件夹）。它不加载 Tauri，只发一条消息就退出，
省掉 `cc-switch.exe` 第二个实例的启动时间（五十多 MB 的程序要初始化到单实例插件那一步才转发）。

PowerToys 键盘管理器：重新映射快捷键 → 选一个快捷键 → 操作选「运行程序」：
- 程序：`cc-switch-open.exe` 的完整路径；不需要参数（也可以继续用 `cc-switch.exe` 加 `--open-claude-desktop`，效果一样，慢一些）；
- 「如果正在运行」一项选「启动另一个实例」。

要换成别的账号，先在 CC Switch 里切到那张卡，再按快捷键。

## 6. 构建与检查

工具链都装在仓库根目录的 `.tool/` 下，不污染本机环境。使用前先 `source .tool/env.sh`，它会设置 `RUSTUP_HOME`、
`CARGO_HOME`、npm/pnpm 的 prefix、store、cache、state，以及 `PATH`。

```sh
source .tool/env.sh
cd src-tauri && cargo fmt && cargo clippy --lib --tests   # 应该 0 警告
cargo test --lib claude_desktop                            # Desktop 相关约 100 个测试
cd .. && pnpm exec tsc --noEmit -p .
pnpm exec vitest run tests/components/ClaudeDesktopSwitch.test.tsx tests/components/SwitchModePanel.test.tsx tests/config/localeParity.test.ts
pnpm tauri build --no-bundle                              # 产物 src-tauri/target/release/cc-switch.exe
cd src-tauri && cargo build --release --bin cc-switch-open # 产物 src-tauri/target/release/cc-switch-open.exe
```

Portable 包：把 `cc-switch.exe`、`cc-switch-open.exe` 和内容如下的 `portable.ini` 一起压成 zip，
输出到 `release-assets/CC-Switch-v<版本>-claude-desktop-accounts-windows-Windows-Portable.zip`。

```ini
# CC Switch portable build marker
portable=true
```

已知的上游原有失败：`import_hermes_providers_from_live_updates_existing_provider_from_live`。在干净的上游上也失败，和本分支无关。

## 7. 已知问题 / 待办

- 「余额查询失败」：还没定位到原因，需要用户提供卡片上失败提示的完整文字。目前查到的情况：
  - 账号卡额度用 `get_claude_account_quota` 查，读的是授权中心里这个号的 Claude Code 登录：
    这个号正好是 `~/.claude` 当前登录的号时读默认槽位，否则读它的账号目录；令牌过期时会跑 `claude -p /usage`；
  - 这条路径在后端只在 CLI 那一步失败时写日志，直接查询失败不写日志。可以考虑补上日志。
- 用户把 Desktop 账号卡绑在了和默认数据目录同一个号上。这种情况下账号卡会另开一个空目录，要重新登录，意义不大。
  可以考虑在界面上提示，或者直接用默认目录。
- 「此电脑」会话：在原件所在的目录里删会话会删掉共用的工作区，见 4.5 的风险一条。
- `cc-switch-open.exe` 和直接激活窗口没有在真实环境里试过（不能替用户启动 Desktop）；拿不到前台时会退回旧办法。
- 没有在真实 Desktop 上做端到端测试（用户要求不要自动启动、关闭 Claude）。Windows 部分只用单元测试和只读探测验证过。
- `--user-data-dir` 和 `--claude-first-party-sign-in` 都不是公开接口，Desktop 升级后要复查第 4.1 节。
