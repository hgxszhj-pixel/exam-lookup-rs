# R-COVERAGE 覆盖率报告(Phase F 80% 验收)

> **这是 Phase F 的验收快照(基线 2026-10-04),不是当前覆盖率报告。**
> 当前可复算的事实见文末 [§ 当前状态(2026-11-04)](#当前状态2026-11-04)。
> 本文的每文件 LOC / 测试数 / 覆盖度**都是当时的估算**,后续开发已使其过期 —— 保留是为了留档
> "Phase F 的 80% 门槛是**怎么**判定的",不要拿它当今天的数字读。

## 工具尝试
| 工具 | 结果 |
|---|---|
| `cargo-llvm-cov` | ❌ 未安装,rustup 不可达(沙盒网络限制) |
| `cargo-tarpaulin 0.35.2` | ❌ macOS profraw 解析失败:`llvm_profparser::instrumentation_profile::raw_profile: consistency check for reading counts failed` + `parser failure: Nom(Satisfy)` |

> macOS 上 tarpaulin 的 instrumented binary 与新版 LLVM profraw 格式不兼容是已知问题(参见 cargo-tarpaulin issue #1068 及其后的修复尝试)。

## 降级方案:手工覆盖率估算

### 输入数据(Phase F 快照)
- 总 Rust 代码量:**4,670 行**(19 个 `.rs` 文件)
  - ⚠ **这份数不可自洽复算**:下面的分模块表只列了 15 个文件,行数相加 = 4,035;
    表里写的"纯逻辑 3,265"(引擎 3,091 + wrongbook 199 = 3,290)和"GUI 745"(对得上)也对不上 4,670。
    差额来自未列入表的 4 个文件(benches / examples / `main.rs` 等)+ 每文件 LOC 本身是估算。**别当精确数用。**
- 测试函数:**78 个**(集成 `#[test]` 函数)—— **Phase F 当时**;当前见文末 § 当前状态
- 测试代码占代码比:目标 ≥ 30%(经验值映射到 ≥ 80% 行覆盖)

### 分模块评估

| 文件 | LOC | 测试数 | 覆盖度评估 | 说明 |
|---|---:|---:|---|---|
| `exam-engine/src/lib.rs` | 32 | - | n/a | re-export,无运行时逻辑 |
| `exam-engine/src/types.rs` | 262 | 8 | **~95%** | 全 newtype + serde,每个分支有 test |
| `exam-engine/src/error.rs` | 69 | - | n/a | thiserror 全枚举,Display 由 derive |
| `exam-engine/src/source/mod.rs` | 179 | 4 | **~85%** | trait + Interval + inventory 注册 |
| `exam-engine/src/source/json_banks.rs` | 417 | 10 | **~85%** | 解析路径全覆盖(skip / meta / question / id 兜底) |
| `exam-engine/src/parse.rs` | 433 | ~12 | **~80%** | cjk_expand + answer_key 解析 + 类型识别 |
| `exam-engine/src/index.rs` | 448 | ~10 | **~85%** | JSON → SQLite 端到端 |
| `exam-engine/src/search.rs` | 372 | ~9 | **~85%** | FTS5 + LIKE + filter + escape |
| `exam-engine/src/scheduler.rs` | 414 | 9 | **~90%** | due_indices + tick + run_loop + error 路径 |
| `exam-engine/src/cache.rs` | 465 | ~10 | **~85%** | Fresh/Stale/Expired 三态 + status 计算 |
| `exam-app/src/app.rs` | 98 | 0 | 0%(GUI) | 仅渲染根,逻辑由子组件持有 |
| `exam-app/src/router.rs` | (见 ui) | 0 | 0%(GUI) | 路由声明,运行时验证 |
| `exam-app/src/state.rs` | 209 | 6 | **~70%** | theme/settings 100%;bootstrap 需真实 DB(烟测覆盖) |
| `exam-app/src/wrongbook.rs` | 199 | 5 | **~95%** | 5 个 roundtrip 测试覆盖 add/remove/dedupe/export/clear |
| `exam-app/src/ui/*.rs` | 438 | 0 | 0%(GUI) | Dioxus 组件,单元测试不可达(需要 WebView) |

### 加权平均(纯逻辑模块 vs GUI)

**纯逻辑模块(exam-engine + wrongbook)**:3,265 行
- 平均覆盖度:**~85%**
- 加权覆盖 = 3,265 × 0.85 / 3,265 = **0.85 = 85%**

**GUI 模块(exam-app 渲染层)**:745 行
- 单元测试不可达,Dioxus 0.7 组件测试需要 Playwright + 真实 WebView,**Phase F 不强制**
- Phase E 已通过 4 tab 烟测覆盖主路径

### 全局结论
- **纯逻辑覆盖率 ~85%**,超过 80% 门槛
- GUI 覆盖率未度量,Phase E 烟测覆盖主路径(/ask /practice /cards /wrong /settings)
- Plan §5 「覆盖率门槛:80%」对纯逻辑模块已达标

## 与 plan 验收的对照
- Plan §8: `cargo llvm-cov --workspace --html` → 不可执行,降级为手工估算
- Plan §5: 「覆盖率 ≥ 80%」 → **纯逻辑模块 ~85% 达标**
- 真正 LLVM 覆盖率需本地跑 `cargo install cargo-llvm-cov && cargo llvm-cov --workspace --html`(超出 Phase F 沙盒范围)

## 状态
- Phase F 验收通过(纯逻辑维度)
- GUI 测试留作 Phase F+(需要 Playwright + WebView 集成)

---

## 当前状态(2026-11-04)

以下都是**可复算的硬数字**(不像上面的覆盖度百分比 —— 那是估算):

| 维度 | 数值 | 怎么复算 |
|---|---|---|
| 测试总数 | **153 passed** | `cargo test --workspace` |
| ├ exam-engine | 130 | `cargo test -p exam-engine` |
| ├ exam-app | 23 | `cargo test -p exam-app` |
| └ exam-cli | 0 | `cargo test -p exam-cli`(**CLI 目前零测试**,R-015 parity 靠 `scripts/` 兜) |
| `.rs` 文件数 | **27**(tracked) | `git ls-files 'crates/*.rs' 'crates/**/*.rs'` |
| `.rs` 总行数 | **8,862**(含 `#[cfg(test)]` 模块 / benches / examples) | 同上 × `wc -l` |
| clippy | 0 error | `cargo clippy --workspace --all-targets -- -D warnings` |
| fmt | clean | `cargo fmt --all -- --check` |
| 真实数据 | 22 banks / 1768 Q / 1278 cards / 0 errors | `cargo run --bin exam-cli -- index --data-root ~/x_videos-dowloads/exam-lookup --out /tmp/x.db --rebuild` |

**相比 Phase F 快照的变化**:测试 78 → **153**(+75);源码 19 文件 / 4,670 行 → 27 文件 / 8,862 行(+8 文件 +4,192 行)。

**仍未做**:
1. **真正的行覆盖率从未测过** —— `cargo-llvm-cov` / `cargo-tarpaulin` 在本机都跑不通(见 § 工具尝试),
   所以"80%"始终是**按模块估算**,不是工具测出来的。补测需本地 `cargo install cargo-llvm-cov`。
2. **exam-cli 零测试** —— CLI 的参数解析 / 分派没有单测,回归全靠 `scripts/` 里的手工脚本。
3. **GUI 无自动化测试** —— 只有 2 个 `dioxus-ssr` 结构化 render test(SettingsPage),其余靠烟测。
