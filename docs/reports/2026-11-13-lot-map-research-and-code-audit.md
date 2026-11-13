# lot-map 调研 + exam-lookup-rs 代码审计

**日期**:2026-11-13 · **基线**:`main` @ c06aca2(工作区含 28 个未提交改动)
**方法**:GitHub API + 插件源码阅读 + 3 个隔离子代理(架构 / 工作区 / 风险扫描)+ 主对话逐条复核
**复核状态**:本文所有 ⚠️ 标记项均已由主对话实跑验证(非子代理转述)

---

## 0. 结论速览

| # | 结论 | 证据强度 |
|---|---|---|
| 1 | lot-map 是**会话可视化 mod**,不是代码分析器;它把工具调用画成地图 | 源码 + README |
| 2 | 它有 **3 种显示模式**,你当前只看到最弱的一种(文字行) | 源码 `register.tsx:425-445` |
| 3 | 你看不到地图的根因是**宿主 Orca 的 PTY 不渲染图片**,非配置问题 | `TERM_PROGRAM` 空 + 源码降级逻辑 |
| 4 | 仓库很新(**4 天**)、很活跃:646★ / 73 fork / 0.6.0 | GitHub API |
| 5 | 代码库**当前状态健康**:129 测试全绿、fmt 合规、无编译期不一致 | 实跑 |
| 6 | ⚠️ HEAD 的 `Cargo.lock` **缺 `dioxus-ssr`** → `cargo test --locked` 失败 | **实跑 rc=101** |
| 7 | ⚠️ 28 个脏文件**不是功能改动**,是 `cargo fmt` 全量重排 | token 级 LCS 比对 |
| 8 | ⚠️ 3 个 HIGH 数据丢失级缺陷 + 4 个 MEDIUM 逻辑缺陷 | 逐条实跑复核 |

---

# Part 1 — lot-map 调研

## 1.1 它是什么

`alexeyw/lot-map` — Claude Code 插件,把**会话中每次工具调用 / 模型步 / hook / skill / 子代理**画成一张实时地图:中心是 `CLAUDE`(外环=上下文占用,下方=当前模型),周围 10 个站点(You / Model / Memory / Files / Shell / Network / Browser / Agents / Skills / Hooks),连线是"路线"(最近经过的站点顺序),发亮站点=正在跑,站点外圈是秒针/分针。

- **机制**:`hooks/register.tsx` 用 Claude Code 的 mod SDK(`atom` / `$.ui.open` / `$.ui.blit` / `on('ui.render')`)注册一个名为 `lot-map` 的 pane;数据存在 `atom('board')`,保留最近 80 行(`KEEP = 80`)
- **本质**:它**不做代码分析**,只把你"分析代码时的动作流"可视化。你当前这轮调研在它眼里就是:Files 6 / Shell 4 / Agents 3 / Hooks 15 / Skills 2
- **许可**:MIT,作者 alexeyw

## 1.2 三种显示模式(回答"只有这个吗")

| 模式 | 触发条件 | 形态 |
|---|---|---|
| **A. 矢量地图** | 宿主 surface = desktop(Claude 桌面版 / VS Code) | `<Svg>` 实时绘制,**有动效** ← Image #2 |
| **B. 位图地图** | 终端 + `terminalView: picture` + 终端支持图片(iTerm2 ≥3.5 / kitty / Ghostty) | `<Image>`,插件用 X.Org 公共领域位图字体自己绘 |
| **C. 纯文字行** | 终端 + 图片渲染失败 | 只有"站点标签 + 耗时"行 ← **你现在这个** |

模式 B/C 的分叉在 `hooks/terminal.tsx:59-86`,模式 A/C 的分叉在 `hooks/register.tsx:425-445`。
另外 `terminal-map` 分支(不在 main)有一个作者放弃的**字符画地图**原型:终端里用文字画 10 站点圆环(7 行 × ≤56 列),开关 `terminalMap`。作者理由:"占 7 行屏幕、没有动效"。

## 1.3 你当前为什么只有文字行

```
宿主: /Applications/Orca.app → Orca Terminal Host.app (PTY)
TERM_PROGRAM=(空)   TERM=xterm-256color
```

插件渲染 `<Image>` 时 Orca 只回 alt 文本 → `sent.deny` 含 `alt` → `picture.showsPictures = false`
→ **永久降级**(`hooks/terminal.tsx:95-102`)。

这是**宿主能力问题**,不是你的配置问题 —— 你已经配好了 `terminalView: "picture"`。

## 1.4 怎么拿到完整地图(按推荐度)

1. **换 iTerm**(已装 3.6.11,支持图片协议):`cd 项目 && claude`,终端宽度 ≥144 列 → 模式 B。**最省事**
2. **Claude 桌面版**(`/Applications/Claude.app`)→ 模式 A(SVG),无需图片终端
3. **字符画地图**:`git clone -b terminal-map https://github.com/alexeyw/lot-map && claude --plugin-dir ./lot-map`(单会话试用,原型分支)
4. tmux 需 `set -g allow-passthrough on` + `set -g focus-events on`
5. 换宿主后**必须重启会话**(插件在 `session.start` 注册 pane);`/lot-map` 手动重开面板

## 1.5 GitHub 实时状态(2026-11-13 查)

| 项 | 值 |
|---|---|
| 仓库 | github.com/alexeyw/lot-map · **创建于 2026-11-09(4 天前)** |
| 热度 | **646★ / 73 fork / 4 watch**,MIT |
| 最新 release | **v0.6.0**(2026-11-12T23:58Z) |
| 本地安装版本 | v0.6.0 @ `d3102bf` —— `git fetch` 后与 `origin/main` **完全一致,无遗漏提交** |
| 分支 | `main` / `next`(同 sha,均 = v0.6.0)/ `terminal-map`(原型,落后 main 35) |
| 开放项 | **2 个 PR,0 个 issue** |
| PR #42 | 给 SVG 根节点加 `role="img"`(屏幕阅读器),开放中 |
| PR #49 | 终端 `theme: auto` 跟随 `COLORFGBG`(iTerm2/rxvt/Konsole 会设)→ **未合并前终端恒为深色** |
| 贡献者 | alexeyw ×36,其余 6 人各 1 次(多为 i18n 翻译) |

## 1.6 隐私(PRIVACY.md v0.6.0)

- **不发网络请求、无遥测/分析/崩溃上报、不读文件、不跑进程、不调模型、不往对话注入内容**
- 标签做降敏:文件只留路径**末两段**;URL 只留 **host**(去 login/password/path/query);shell 只留命令短描述;记忆调用只留工具名 + 记录号
- 跨会话只保留"每个目录上次用的模型名"
- 已存进本项目记忆:`lot-map-terminal-picture-gotcha.md`

## 1.7 可调项(`plugin.json` userConfig)

| 选项 | 默认 | 含义 |
|---|---|---|
| `layout` | `you-top` | `you-top` / `you-bottom`(转半圈:你在底,Network+Browser 在顶) |
| `theme` | `auto` | `auto` / `dark` / `light`(终端下 auto 恒深色,见 PR #49) |
| `motion` | `full` | `reduced` = 关闭飞行动画,只淡入淡出 |
| `textSize` | `footnote` | 行流水字号 `caption` / `footnote` / `body` |
| `terminalView` | `text` | `text` / `picture` ← **你已设 `picture`** |
| `cellShape` | `2.4` | 终端图片的单元格高宽比(1.2–4) |
| `background` | `solid` | `solid` / `transparent` ← 你已设 transparent |
| `memoryServers` | `memory` | 你已设 `mempalace` |
| `hookReasons` | `false` | 你已设 `true` = 显示 hook 拒绝原因首行 |
| `language` | `en` | en/ru/es/fr/de/it/pt —— **无中文** |

---

# Part 2 — exam-lookup-rs 代码分析

## 2.1 架构地图

三个 crate:`exam-engine`(lib)+ `exam-cli`(bin)+ `exam-app`(Dioxus 0.7.9 桌面 GUI)。

```
source 注册表(inventory 链接期注册)
  └→ Source::load(data_root) → Vec<LoadResult>
      └→ IndexBuilder::populate → SQLite(q / q_fts / card / card_fts / meta)
          └→ search(conn, &SearchQuery) 三级降级: stem 精确 → FTS5 → LIKE
              ├→ CLI: exam-cli search
              └→ GUI: AppState::search → AskPage / CardsPage
```

- **注册表**:`Source` trait(`source/mod.rs:84`)+ `inventory::submit!`(`json_banks.rs:221`、`crawl_cards.rs:199`)+ `all_sources()`(`source/mod.rs:113`)
  ⚠️ **新增 source 文件必须同时在 `source/mod.rs:16-17` 加 `pub mod xxx;`**,否则链接期根本不会被收集,静默消失
- **调度**:`Scheduler{entries, cache: Arc<Mutex<CacheStore>>, config}`(`scheduler.rs:80`),6 档间隔(`Interval` 枚举 `source/mod.rs:29`),失败源指数退避 5→300s
- **缓存**:`cache.db`(**独立于索引库的第二个 SQLite 文件**),`CacheStatus{Fresh,Stale,Expired}`
- **GUI**:`ShellLayout` 统一外壳,路由 `/`→`/ask`、`/practice`、`/cards`、`/wrong`、`/settings`;`AppState` 用 `Arc<RwLock<>>` 持 settings / `Arc<Mutex<Option<Connection>>>` 持 DB

**3 个值得知道的设计决策**

1. **CJK 分词是"双半约定"**:`rusqlite bundled` 不带 trigram 扩展(R-001 降级),改为写侧 `cjk_expand()` 在相邻汉字间插空格 + 查侧 `build_fts_term` 逐字 OR。**两侧必须同步改,否则 CJK 的 FTS 命中会静默消失**
2. **wrongbook 放在 exam-app 而非 engine**:它是"用户自己的错题",不是语料派生数据;JSON 存在 `~/Library/Application Support/exam-lookup-rs/wrong.json`
3. **无增量更新**:rebuild = `init_schema + truncate + populate`,watcher 每次事件都**全量重读整个语料**

## 2.2 实测验证状态(主对话亲跑)

| 检查 | 结果 |
|---|---|
| `cargo test --workspace` | **129 passed**(4 suites, 2.78s),exit 0 ✅ 与 HANDOFF 声明一致 |
| `cargo fmt --all -- --check` | exit 0 ✅(HEAD 快照则不合规) |
| 公开 API 签名比对 | 26 个 .rs 文件**零签名变更** → 零失效调用点 |

## 2.3 ⚠️ 28 个脏文件的真相

**这不是功能改动。** 全量 `cargo fmt --all` 重排(26 个 .rs,含 `use` 组排序、struct 字面量展开、尾逗号位置、EOF 换行)+ `Cargo.lock` 依赖同步 + HANDOFF.md 加 17 行。

- 分支 `worktree-audit-fix-2026-11-04` @ f4735c7(21 commits,已有 worktree)的 HANDOFF **已判定**:这 28 个文件"合并前丢掉",只有 `docs/platform-training-docs.{md,json}` 需要抢救成独立 commit
- 真实的工作(P0-1/P1-1..P1-4/P2-3/P2-5/P2-7 修复、`R-016-code-audit`、`ci.yml`、`rust-toolchain.toml` 锁 1.99.1、`perf_budget.rs`、`parity_check.py`)**都在那个分支上,不在 main**

## 2.4 ⚠️ 已复核缺陷

### HIGH

**H1. `Cargo.lock` 缺 `dioxus-ssr` → `--locked` 全线失败**
`crates/exam-app/Cargo.toml` 声明了 `[dev-dependencies] dioxus-ssr = "0.7.9"`(Phase P 引入),但 HEAD 的 `Cargo.lock` **既没有该 package,也没把它列进 exam-app 的依赖**。
> **实跑证明**:HEAD 快照 `cargo metadata --locked` → **rc=101** `cannot update the lock file ... because --locked was passed`;工作区(修正后的 lock)→ **rc=0** ✅
> 后果:`cargo test --locked` / `cargo build --locked` / CI 的 `--locked` 检查在干净 checkout 上直接失败;SSR 测试(`ui/settings.rs:230-296`)无法编译。
> ⚠️ 本地 `cargo test` 能过是因为工作区的 lock 已被修正 —— **这个 bug 藏在你还没提交的改动里**。

**H2. `index.rs:144-148` rebuild 先 truncate 后 populate → 失败即空库**
`rebuild()` = `init_schema()` → `truncate()`(独立事务,已提交)→ `populate()`(另一个 `unchecked_transaction`)。
> 场景:watcher 在编辑器写一半时触发重建 → `load_one_bank` 解析失败 → `rebuild` 返回 Err → **磁盘上的索引已被清空**,搜索全部 0 命中,直到下次成功重建。`data_root` 拼错同样触发。

**H3. `wrongbook.rs:152-159` `write_atomic` 是假原子写**
```rust
fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    ...
    std::fs::write(path, content)   // 直接截断重写,无 temp + rename
}
```
> 场景:写 `wrong.json` 途中进程被杀 → 文件截断 → 下次启动 `wrongbook.rs:86` 的 `.and_then(|s| serde_json::from_str(...).ok())` **静默变成空错题本**,下次 add 就把文件覆盖成只剩新条目 → 历史错题永久丢失。

**H4. `state.rs:345` watcher 线程"复活"→ 旧目录数据覆盖新索引**
`stop_watcher()` 把共享的 `Arc<AtomicBool>` 置 false 并 drop JoinHandle(线程 detach);`ensure_watcher()` 又把**同一个 flag** 置 true(旧线程持有 clone)。`sync_watcher(true)` = stop + ensure,所以旧线程可能**永远看不到 false**。
> 线程体拿的是**启动时快照**(源码注释自己写的 "snapshot 启动时的路径",`state.rs:378-392`)。
> 场景:用户在 Settings 改 `data_root`(db_path 不变)→ 旧线程仍监听**旧目录**且用旧快照重建**同一个 db_path** → **新索引被旧目录的数据覆盖**;行为还依赖 500ms `recv_timeout` 的竞态,不确定复现。

### MEDIUM

**M1. `search.rs:186` LIKE 转义了却没加 `ESCAPE` 子句 → 搜 `%`/`_` 静默 0 命中**
`escape_like` 产出 `\%` / `\_`,但 SQL 里没有 `ESCAPE` —— SQLite 的双参数 `LIKE` **没有默认转义字符**。
> **实跑**:`sqlite3` → `'a50%b' LIKE '%50\%%'` = **0**,`'a50%b' LIKE '%50%%'` = 1。用户搜 `50%` 或 `file_name` → 转义反被当字面量 → 静默无结果。

**M2. `index.rs:268/291` `q_fts`/`card_fts` 用裸 `INSERT`,`q` 用 `INSERT OR REPLACE` → 同 id 出现重复行**
`q_fts` 的 `id` 是 `UNINDEXED`(无唯一约束),裸 INSERT 两次同 id = 两行 → `search.rs:160` 的 JOIN 对同一题返回 2 条;`stats.questions` 也多计。

**M3. `ui/practice.rs:109` 判题用 `contains` → 多选题只选一半被判"正确"**
`cur.correct.to_uppercase().contains(&pick.to_uppercase())`,而 `q.answer` = `answer_keys.join("/")`(`index.rs:236`)→ 多选答案是 `"A/B"`。
> 场景:多选题正确答案是 A/B,用户只选 A → `"A/B".contains("A")` = true → **判为答对,且不进错题本**。

**M4. `ui/settings.rs:204` / `ui/ask.rs:40` 阻塞调用直接跑在 Dioxus UI 线程**
- 重建索引(读+解析 ~16MB JSON + 数千行 SQLite 插入)在 `onclick` 里**同步**跑 → 窗口整个卡死,且 `rebuilding.set(true)` 来不及绘制("重建中…"永远看不见)
- 搜索**每敲一个键**同步跑一次 FTS5/LIKE,无防抖;LIKE 降级是 `q` 表全扫描 → 输入框卡顿(`ui/cards.rs:41` 同病)

### LOW(节选)

- `state.rs:91` 默认 `data_root` 硬编码 `/Users/hgxszhj/x_videos-dowloads/exam-lookup`,`db_path` 是相对路径 `"questions.db"` → 从 .app bundle 启动时 CWD=`/` → 建库失败
- `state.rs:435/464` 设置读写的错误一律吞掉回默认值;`settings.json` 截断 → 静默回落到**开发者专属默认路径**,且下次保存会把用户真实配置覆盖掉
- `scheduler.rs:132/182/213` 三处 `.expect("cache mutex poisoned")` → 中毒即 panic 打死调度线程
- `scheduler.rs:134` 缓存读失败被 `.ok().flatten()` 压成"无缓存" → 每 tick 全量重载所有源(每 500ms 一次全量解析),**无日志**
- `cache.rs:96` `should_refresh` 的 `Interval` 参数被命名成 `_interval`(直接忽略)→ 改源的间隔档位对已缓存条目无效
- `ui/practice.rs:37` `SELECT ... LIMIT 20` 无 `ORDER BY`/`OFFSET` → 练习永远是同样 20 题同样顺序
- `source/mod.rs:132` `metadata()` 后 `read()` 是 TOCTOU → 64MiB 上限可被绕过
- `state.rs:306` `ensure_watcher` 的 `is_some()` 检查与赋值未持锁 → 并发可起两个线程;`state.rs:324` 线程持有的 AppState clone 与 JoinHandle 构成 `Arc` 引用环 → 连接不释放

### 无问题的方面(负面结论也重要)

- **SQL 注入:0 处**。51 个 SQL 语句点全部走 `params!` / `?1` 绑定,用户输入零拼接
- **字节切片 panic:0 处**。`parse.rs` 所有切片走 `char_indices()`(字符边界安全)
- **`unsafe` 块:0 个**
- 纯逻辑层(parse / search / cache / scheduler / watcher)测试覆盖扎实

## 2.5 ⚠️ 文档漂移(实测)

1. **HANDOFF.md:55** 写 "229 个文档",但 `docs/platform-training-docs.json` 的 `docs` 数组实为 **187** 条,`.md` 的表格行也是 187 行(10 个分区 83+20+14+1+7+2+3+9+23+25 = 187);而 **HANDOFF.md:150** 自己写的是 "187 条清单" → **同一个 HANDOFF 里两处自相矛盾**
2. **R-014 从未存在**:`docs/reports/` 有 R-001..R-013、R-015,唯独没有 R-014;`git log --all --diff-filter=A -- docs/reports/R-014*` 为空。HANDOFF.md:33 声称 "R-001..R-015"
3. `.omc/` 未被 main 的 `.gitignore` 覆盖(分支已修)

## 2.6 测试盲区

| 已有覆盖 | 盲区 |
|---|---|
| parse 21 / search 17 / index 10 / cache 15 / scheduler 15 / watcher 8 / settings SSR 2 | `ask.rs`、`cards.rs`、`practice.rs`、`wrong.rs`、`shell.rs`、`router.rs`、`app.rs` **零测试模块** → 上述 M3/M4 及所有 UI 缺陷不可回归检测 |
| `rebuild_fails_fast_when_subdir_missing` 只断言错误类型,**不检查 DB 事后状态** → H2 隐形 | `escape_like_handles_wildcards` 只断言转义**字符串**,不测 SQL 语义 → M1 隐形 |
| watcher 测试只断言 slot 状态,**不验证旧线程是否真的退出** → H4 隐形 | `wrongbook.rs` 的 2 个测试只做 `WrongEntry` 往返,**从不碰 `write_atomic` / 损坏恢复** → H3 隐形 |

## 2.7 建议动作(按性价比)

| 优先 | 动作 | 成本 |
|---|---|---|
| P0 | **先处理 lock 漂移**:把工作区 `Cargo.lock` 的 `dioxus-ssr` 修复单独 cherry-pick 到 main(或直接推到分支),别让它随 fmt 大扫除一起被丢掉 | 分钟级,单文件 |
| P0 | 在合并 `worktree-audit-fix-2026-11-04` 前按分支 HANDOFF 的判定**丢弃这 28 个脏文件**(`git checkout -- .`),只抢救 `docs/platform-training-docs.{md,json}` | 分钟级 |
| P1 | H2:`rebuild` 把 truncate+populate 合并进**同一个事务** | 小,`index.rs:144-148` |
| P1 | H3:`write_atomic` 改成 temp + rename;`wrongbook.rs:86` 解析失败要报错而不是回空 | 小 |
| P1 | M1:给 LIKE 加 `ESCAPE '\'` 子句,并补一条"搜 `%` 能命中"的测试 | 小 |
| P1 | M3:判题改成**集合相等**(按 `/` 切分后比较),不靠 `contains` | 小 |
| P2 | H4:watcher 用**独立于共享 flag 的停止信号**(如每个线程一份 `Arc<AtomicBool>`),或 stop 后 `join()` 而非 detach | 中 |
| P2 | M4:重建/搜索移出 UI 线程(`spawn_blocking`)+ 搜索加防抖 | 中 |
| P2 | 修正 HANDOFF 的 229→187、补 R-014 缺口说明 | 分钟级 |
| P3 | 清理 `state.rs:91/95` 的硬编码路径与相对 db_path | 小 |

## 2.8 现有验证资产(可复用的证据)

`scripts/dump_parity.sh` + `scripts/dump_banks.py`(已提交,1760 题 0.00% diff,`R-015-dump-parity.md`)、
`crates/exam-engine/benches/perf.rs`(6 个 divan 闸门)、`crates/exam-engine/examples/smoke_realdata.rs`。
分支另有 `scripts/parity_check.py`(298 行,main 没有)、`rust-toolchain.toml` 锁 1.99.1、`.github/workflows/ci.yml`。
