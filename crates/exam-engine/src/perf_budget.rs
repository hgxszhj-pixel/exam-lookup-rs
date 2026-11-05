//! Perf 预算门禁 —— 把 R-PERF.md 里的阈值变成**可执行的断言**。
//!
//! **背景(2026-11-04 审计 P2-6)**:`docs/reports/R-PERF.md` 写着
//! 「Perf 预算超过即触发 CI failure」,但阈值只活在 Markdown 里 ——
//! `crates/exam-engine/benches/perf.rs` 的 6 个 bench **零断言**。
//! 那是一道**永远不会触发**的假门禁:阈值和代码之间没有连线。
//!
//! **修法**:阈值收进代码(本模块),`benches/perf.rs` 每个 bench 跑完
//! 用实测耗时(P25,见 [`GATE_QUANTILE`])对一次账,超限 → 记失败 → `cargo bench` 退出码 1。
//!
//! **阈值来源**:`docs/reports/R-PERF.md` §「Perf 预算(发版前 gate)」表
//! (2026-10-04 在 macOS 24.6.0 / Apple Silicon 上实测 + 2026-11-04 按 P25 重新标定)。
//! 文档里的表是**说明**,本模块的常量才是**单一真相来源** ——
//! 两边不一致时以本模块为准(改了阈值要同步改文档)。
//!
//! **共享 runner 的伸缩**:`EXAM_PERF_BUDGET_SCALE` 环境变量统一缩放阈值。
//! 绝对耗时是硬件相关的 —— GitHub 共享 runner 比开发机慢 2~3 倍,
//! 直接用开发机阈值会**必然**误红。缩放系数让 CI 能按 runner 调档,
//! 而门禁本身的判定逻辑一点没变(仍然是「超过即红」)。

use thiserror::Error;

/// 单项 perf 预算。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfBudget {
    /// 指标名 —— 与 R-PERF.md 预算表的行名一致。
    pub name: &'static str,
    /// 容忍上限(毫秒)。
    pub limit_ms: f64,
}

impl PerfBudget {
    /// 按系数缩放阈值(给共享 CI runner 用)。
    pub fn scaled(self, scale: f64) -> Self {
        PerfBudget {
            name: self.name,
            limit_ms: self.limit_ms * scale,
        }
    }
}

/// 预算超限。
#[derive(Debug, Clone, PartialEq, Error)]
#[error("perf 预算超限:{name} 实测 {measured_ms:.3} ms > 上限 {limit_ms:.3} ms")]
pub struct BudgetExceeded {
    pub name: &'static str,
    pub limit_ms: f64,
    pub measured_ms: f64,
}

/// 判定实测耗时是否在预算内。
///
/// 超限返回 `Err` —— 这就是「门禁能变红」的判定点。
pub fn check(budget: &PerfBudget, measured_ms: f64) -> Result<(), BudgetExceeded> {
    if measured_ms > budget.limit_ms {
        Err(BudgetExceeded {
            name: budget.name,
            limit_ms: budget.limit_ms,
            measured_ms,
        })
    } else {
        Ok(())
    }
}

/// 断言版:超限即 panic。
///
/// 注意:bench 里**不要**用这个 —— divan 会 `catch_unwind` 掉 bench 函数里的
/// panic 并**继续跑下一个 bench**,进程退出码仍是 0(见 `benches/perf.rs` 的注释)。
/// bench 用的是「记失败 + 退出码 1」,这个 panic 版留给纯 Rust 场景。
pub fn assert_within(budget: &PerfBudget, measured_ms: f64) {
    if let Err(e) = check(budget, measured_ms) {
        panic!("{e}");
    }
}

/// 门禁用的统计量:**P25**(25 分位)。
///
/// **为什么不是中位数**(2026-11-04 实测,这台机器上):
///
/// | 版本(file-backed,400 行 fixture) | 中位 | min |
/// |---|---|---|
/// | 修复后 | 18.8 / 21.2 / 21.8 ms | 16.6 / 16.7 / 17.4 ms |
/// | 回归版(`RETURNING`) | 31.5 / 32.7 / 32.8 ms | 29.4 / 29.9 / 30.0 ms |
///
/// 机器繁忙时**整个分布会平移**(某一轮的中位 30.4 ms,另一轮 18.8 ms,
/// 差 60%),而 min / 低分位几乎不动(±3%)。
/// 也就是说:中位量的是「这台机器当下的忙碌程度」,P25 量的是
/// 「这份代码在这台机器上**能跑多快**」—— 后者才是回归检测想要的量。
/// 换句话说:中位数会被邻居的 CPU 占用误伤,低分位不会。
///
/// **诚实的补充**:P25 只挡得住**批内**的个别慢样本。**批间**的负载漂移
/// (这台机器 load average 常年 3~4,邻居一忙整批都慢)任何统计量都挡不住 ——
/// 那部分余量靠**阈值本身留的 margin**(见各 `PerfBudget` 的标定注释)。
/// 换句话说:这道门禁抓的是 **≥1.3~1.4× 的回归**,不是 5% 的漂移。
pub const GATE_QUANTILE: f64 = 0.25;

/// 分位数(线性插值,同 numpy 默认)。`q` 取 0 → 最小值,1 → 最大值。
///
/// 空样本返回 `None`;`q` 不在 `[0, 1]` 内返回 `None`。
pub fn percentile(samples: &[f64], q: f64) -> Option<f64> {
    if samples.is_empty() || !(0.0..=1.0).contains(&q) {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        return Some(sorted[lo]);
    }
    let frac = pos - lo as f64;
    Some(sorted[lo] + (sorted[hi] - sorted[lo]) * frac)
}

/// 中位数。空样本返回 `None`;偶数个样本取中间两个的均值。
pub fn median(samples: &[f64]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[mid])
    } else {
        Some((sorted[mid - 1] + sorted[mid]) / 2.0)
    }
}

/// 解析缩放系数:非法 / 非正数 → 1.0(不缩放)。
pub fn parse_scale(raw: &str) -> f64 {
    match raw.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => v,
        _ => 1.0,
    }
}

/// 从环境变量 `EXAM_PERF_BUDGET_SCALE` 读缩放系数,缺省 1.0。
pub fn scale_from_env() -> f64 {
    std::env::var("EXAM_PERF_BUDGET_SCALE")
        .map(|raw| parse_scale(&raw))
        .unwrap_or(1.0)
}

// -------- 预算表 --------
//
// 与 `docs/reports/R-PERF.md` §「Perf 预算(发版前 gate)」一一对应。

/// `build_index` —— 200 题 + 200 问答 重建全索引。
///
/// **2026-11-04 重新标定:35 ms → 28 ms。**
/// 35 ms 是 2026-10-04 按**中位数**定的(基线 23.28 ms + 50% 余量)。
/// 换到 P25 之后它太松 —— 实测(同一台机器、同一 fixture、同一 bench 上下文):
///
/// | 版本 | P25 | min |
/// |---|---|---|
/// | 修复后(upsert + SELECT rowid) | 22.0 ms | 20.1 ms |
/// | 回归版(写回 `RETURNING`) | 33.3 ms | 30.9 ms |
///
/// 35 ms 的上限**放回归版从底下溜过去了**。28 ms 落在两者之间:
/// 修复后余量 27%,回归版超限 19% —— 两边都有实测支撑。
/// 完整数据见 `docs/reports/R-PERF.md` §「门禁能抓住什么、抓不住什么」。
pub const BUILD_INDEX: PerfBudget = PerfBudget {
    name: "build_index",
    limit_ms: 28.0,
};
/// `search_fts` —— 单 CJK 关键词 bm25。
pub const SEARCH_FTS: PerfBudget = PerfBudget {
    name: "search_fts",
    limit_ms: 2.5,
};
/// `search_like` —— LIKE 兜底路径(全表 `LIKE '%needle%'` 扫描)。
pub const SEARCH_LIKE: PerfBudget = PerfBudget {
    name: "search_like",
    limit_ms: 3.0,
};
/// `search_cards_fts` —— card 表 FTS5。
///
/// 2026-11-04 新增:R-PERF 的预算表原本只覆盖 5 项,`search_cards_fts` 有 bench
/// 却没有阈值 —— 补一个同量级的上限(实测中位 1.375 ms → 上限 2.5 ms)。
pub const SEARCH_CARDS_FTS: PerfBudget = PerfBudget {
    name: "search_cards_fts",
    limit_ms: 2.5,
};
/// `load_json_banks` —— JsonBanksSource parse + split。
///
/// **2026-11-04 重新标定:1.5 ms → 2.5 ms。**
/// 这是纯 CPU 的 serde_json 解析,在机器繁忙时波动很大:实测 P25 在
/// **0.91 ms ~ 1.76 ms** 之间(同一台机器、同一 fixture,只因邻居进程
/// 占了 CPU)。1.5 ms 的上限落在波动带中间 → 门禁会**误红**。
/// 2.5 ms 给最差观测值留 42% 余量,仍然能抓住 ≥1.4× 的回归。
pub const LOAD_JSON_BANKS: PerfBudget = PerfBudget {
    name: "load_json_banks",
    limit_ms: 2.5,
};
/// `load_crawl_cards` —— CrawlCardsSource parse + split。
pub const LOAD_CRAWL_CARDS: PerfBudget = PerfBudget {
    name: "load_crawl_cards",
    limit_ms: 1.2,
};

/// 全部预算(供自检 / 文档生成用)。
pub const ALL_BUDGETS: &[PerfBudget] = &[
    BUILD_INDEX,
    SEARCH_FTS,
    SEARCH_LIKE,
    SEARCH_CARDS_FTS,
    LOAD_JSON_BANKS,
    LOAD_CRAWL_CARDS,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn gate_passes_when_median_within_limit() {
        let b = PerfBudget {
            name: "demo",
            limit_ms: 10.0,
        };
        assert!(check(&b, 9.999).is_ok());
        // 恰好等于上限不算超限(门禁判「超过」,不是「达到」)。
        assert!(check(&b, 10.0).is_ok());
    }

    #[test]
    fn gate_fails_when_median_exceeds_limit() {
        // 这是「门禁能变红」的证据 —— 阈值不再是 Markdown 里的死文字。
        let b = PerfBudget {
            name: "demo",
            limit_ms: 10.0,
        };
        let err = check(&b, 10.001).unwrap_err();
        assert_eq!(err.name, "demo");
        assert_eq!(err.limit_ms, 10.0);
        assert_eq!(err.measured_ms, 10.001);
        // 错误信息要能直接看懂超了多少。
        let msg = err.to_string();
        assert!(msg.contains("demo"), "{msg}");
        assert!(msg.contains("10.001"), "{msg}");
    }

    #[test]
    fn assert_within_panics_when_over_limit() {
        let b = PerfBudget {
            name: "demo",
            limit_ms: 1.0,
        };
        let panicked = std::panic::catch_unwind(|| assert_within(&b, 2.0));
        assert!(panicked.is_err(), "超限必须 panic");
        let ok = std::panic::catch_unwind(|| assert_within(&b, 0.5));
        assert!(ok.is_ok(), "未超限不能 panic");
    }

    #[test]
    fn median_of_odd_count_is_the_middle_value() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
    }

    #[test]
    fn median_of_even_count_averages_the_two_middle_values() {
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]), Some(2.5));
    }

    #[test]
    fn median_of_empty_is_none() {
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn parse_scale_rejects_garbage_and_non_positive() {
        assert_eq!(parse_scale("2.5"), 2.5);
        assert_eq!(parse_scale(" 3 "), 3.0);
        // 垃圾输入 / 非正数 / NaN / inf 一律退回 1.0(不缩放),
        // 不能让一个拼错的环境变量把门禁悄悄废掉。
        assert_eq!(parse_scale(""), 1.0);
        assert_eq!(parse_scale("abc"), 1.0);
        assert_eq!(parse_scale("0"), 1.0);
        assert_eq!(parse_scale("-1"), 1.0);
        assert_eq!(parse_scale("NaN"), 1.0);
        assert_eq!(parse_scale("inf"), 1.0);
    }

    #[test]
    fn scaled_budget_scales_only_the_limit() {
        let b = PerfBudget {
            name: "demo",
            limit_ms: 10.0,
        };
        let s = b.scaled(2.0);
        assert_eq!(s.name, "demo");
        assert_eq!(s.limit_ms, 20.0);
    }

    #[test]
    fn every_budget_is_positive_and_uniquely_named() {
        let mut names = HashSet::new();
        for b in ALL_BUDGETS {
            assert!(b.limit_ms > 0.0, "{} 的阈值必须为正", b.name);
            assert!(names.insert(b.name), "预算名重复:{}", b.name);
        }
        assert_eq!(names.len(), ALL_BUDGETS.len());
    }

    #[test]
    fn all_budgets_gate_can_go_red() {
        // 逐项证明「阈值不是摆设」:把实测值设成阈值的 2 倍,必须判红。
        for b in ALL_BUDGETS {
            let over = b.limit_ms * 2.0;
            assert!(check(b, over).is_err(), "{} 超限却判绿 —— 门禁失效", b.name);
            assert!(check(b, b.limit_ms).is_ok(), "{} 边界值判红", b.name);
        }
    }

    // -------- 分位数 --------

    #[test]
    fn percentile_of_empty_is_none() {
        assert_eq!(percentile(&[], 0.25), None);
    }

    #[test]
    fn percentile_rejects_out_of_range_q() {
        assert_eq!(percentile(&[1.0, 2.0], -0.1), None);
        assert_eq!(percentile(&[1.0, 2.0], 1.1), None);
        assert_eq!(percentile(&[1.0, 2.0], f64::NAN), None);
    }

    #[test]
    fn percentile_endpoints_are_min_and_max() {
        let xs = [5.0, 1.0, 3.0, 2.0, 4.0];
        assert_eq!(percentile(&xs, 0.0), Some(1.0));
        assert_eq!(percentile(&xs, 1.0), Some(5.0));
    }

    #[test]
    fn percentile_interpolates_like_numpy() {
        // 5 个样本,q=0.25 → pos = 0.25 * 4 = 1.0 → 正好落在第 2 个样本上。
        let xs = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&xs, 0.25), Some(2.0));
        // q=0.5 → pos = 2.0 → 中位数。
        assert_eq!(percentile(&xs, 0.5), Some(3.0));
        // q=0.3 → pos = 1.2 → 在 2.0 与 3.0 之间插值 20%。
        let p = percentile(&xs, 0.3).unwrap();
        assert!((p - 2.2).abs() < 1e-9, "实际 {p}");
    }

    #[test]
    fn percentile_is_robust_to_a_few_slow_samples() {
        // 这就是选 P25 而不是中位的理由:几个慢样本(机器被抢占)不该影响判定。
        let fast: Vec<f64> = (0..45).map(|i| 10.0 + i as f64 * 0.01).collect();
        let mut noisy = fast.clone();
        noisy.extend([100.0, 120.0, 90.0, 110.0, 95.0, 105.0]);
        let p25 = percentile(&noisy, GATE_QUANTILE).unwrap();
        assert!(p25 < 15.0, "P25 不该被几个慢样本带偏,实际 {p25}");
        // 中位会被带偏 —— 留着这条注释说明为什么不用中位。
        let med = median(&noisy).unwrap();
        assert!(
            med > p25,
            "中位({med})应高于 P25({p25})—— 这正是我们不用中位的原因"
        );
    }

    #[test]
    fn gate_quantile_is_in_range() {
        assert!((0.0..1.0).contains(&GATE_QUANTILE));
    }

    /// 门禁必须能抓住 2026-11-04 那类回归(实测数据,见 R-PERF.md)。
    ///
    /// 同一台机器、同一 fixture、同一 bench 上下文:
    /// 修复后 P25 = 22.0 ms,回归版(`RETURNING`)P25 = 33.3 ms。
    /// 28 ms 的阈值落在两者之间 —— 修好的代码过,回归的代码红。
    #[test]
    fn build_index_threshold_separates_fixed_from_regressed() {
        let fixed_p25 = 22.0;
        let regressed_p25 = 33.3;
        assert!(
            check(&BUILD_INDEX, fixed_p25).is_ok(),
            "修复后的代码不该被误红"
        );
        assert!(
            check(&BUILD_INDEX, regressed_p25).is_err(),
            "回归版的代码必须被判红 —— 否则这道门禁又是个摆设"
        );
    }

    /// `load_json_banks` 的阈值必须扛得住机器繁忙时的 CPU 抢占,
    /// 同时仍然抓得住 ≥1.4× 的回归(实测波动带 0.91 ~ 1.76 ms)。
    #[test]
    fn load_json_banks_threshold_survives_contention() {
        assert!(
            check(&LOAD_JSON_BANKS, 1.76).is_ok(),
            "机器繁忙时的实测值不该被判红"
        );
        assert!(
            check(&LOAD_JSON_BANKS, LOAD_JSON_BANKS.limit_ms * 1.4).is_err(),
            "1.4× 回归必须被判红"
        );
    }
}
