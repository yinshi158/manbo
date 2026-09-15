//! 「键 → 计数」表的三方合并：`merged[键][列] = remote + (local − base)`，负数截到 0。
//!
//! 删除（`forget` 清零）与衰减（减半）因此作为负增量传播到其他设备；同一份快照因为基准跟着
//! 走，重复合并不会重复计数。输出按键排序（键列升序），与学习器落盘的按次数排序不同——加载
//! 只认行不认顺序，顺序在壳下一次落盘时自愈。

use std::collections::BTreeMap;

use super::data_lines;

/// 键列怎么取。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KeyColumns {
    /// 除最后一列（计数）外的全部列：`user.tsv` / `user-ngram.tsv`（3、4 列两种行都成立）/
    /// `user-choices.tsv` / `user-english.tsv` / `user-typos.tsv` / `user-words.tsv`。
    AllButLast,

    /// 第一列是键、后面全是计数列：`usage.tsv`（日期 → 汉字 / 中文词 / 英文词 / 上屏次数）。
    First,
}

/// 一侧解析结果：键 → 各计数列。
type Rows = BTreeMap<Vec<String>, Vec<u64>>;

/// 按公式合并三侧；`cap` 给并集表封顶（词频恒 100，两台设备各自新增同一个词不翻倍）。
pub(super) fn merge(
    key_columns: KeyColumns,
    cap: Option<u64>,
    base: &str,
    local: &str,
    remote: &str,
) -> String {
    let base = parse(key_columns, base);
    let local = parse(key_columns, local);
    let remote = parse(key_columns, remote);
    let mut keys: std::collections::BTreeSet<&Vec<String>> = base.keys().collect();
    keys.extend(local.keys());
    keys.extend(remote.keys());
    let mut out = String::new();
    for key in keys {
        let width = base
            .get(key)
            .map(Vec::len)
            .max(local.get(key).map(Vec::len))
            .max(remote.get(key).map(Vec::len))
            .unwrap_or(0);
        let mut values = Vec::with_capacity(width);
        for column in 0..width {
            let count = |rows: &Rows| -> i64 {
                rows.get(key)
                    .and_then(|values| values.get(column))
                    .copied()
                    .unwrap_or(0) as i64
            };
            let delta = count(&local) - count(&base);
            let mut merged = count(&remote) + delta;
            if merged < 0 {
                merged = 0;
            }
            if let Some(cap) = cap {
                merged = merged.min(cap as i64);
            }
            values.push(merged as u64);
        }
        // 全零行丢弃：删除与衰减传播的终点，不往文件里堆空壳
        if values.iter().all(|value| *value == 0) {
            continue;
        }
        out.push_str(&key.join("\t"));
        for value in values {
            out.push('\t');
            out.push_str(&value.to_string());
        }
        out.push('\n');
    }
    out
}

/// 解析一侧：键列 + 计数列；计数列有不是数字的整行跳过；同一侧出现重复键，各列累加
/// （与 `usage.tsv` 加载「同一天两次就加起来」的语义一致）。
fn parse(key_columns: KeyColumns, text: &str) -> Rows {
    let mut rows = Rows::new();
    for line in data_lines(text) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let (key, counts) = match key_columns {
            KeyColumns::First => (
                vec![fields[0].to_owned()],
                fields[1..]
                    .iter()
                    .map(|field| field.trim().parse::<u64>())
                    .collect::<Result<Vec<_>, _>>(),
            ),
            KeyColumns::AllButLast => (
                fields[..fields.len() - 1]
                    .iter()
                    .map(|field| (*field).to_owned())
                    .collect(),
                fields[fields.len() - 1]
                    .trim()
                    .parse::<u64>()
                    .map(|count| vec![count]),
            ),
        };
        let Ok(counts) = counts else {
            tracing::debug!(line, "同步合并：计数列不是数字，跳过这一行");
            continue;
        };
        let entry = rows.entry(key).or_default();
        for (index, count) in counts.into_iter().enumerate() {
            if entry.len() <= index {
                entry.resize(index + 1, 0);
            }
            entry[index] += count;
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use crate::registry::MergePolicy;

    /// 走 merge/mod.rs 的分发入口（`merge` 在本文件里是按 KeyColumns 的底层版本）。
    fn dispatch(policy: MergePolicy, base: Option<&str>, local: &str, remote: &str) -> String {
        crate::merge::merge(policy, base, Some(local), Some(remote))
    }

    #[test]
    fn sums_independent_deltas_from_both_sides() {
        // 基准 10；本机 +5，另一台 +8 → 23
        let merged = dispatch(
            MergePolicy::Counters,
            Some("微博\t10\n"),
            "微博\t15\n",
            "微博\t18\n",
        );
        assert_eq!(merged, "微博\t23\n");
    }

    #[test]
    fn first_sync_without_base_adds_both_sides() {
        let merged = crate::merge::merge(
            MergePolicy::Counters,
            None,
            Some("吧\t3\n"),
            Some("把\t4\n"),
        );
        assert_eq!(merged, "吧\t3\n把\t4\n");
    }

    #[test]
    fn repeated_merges_are_idempotent() {
        let base = "微博\t10\n";
        let local = "微博\t15\n";
        let remote = "微博\t18\n";
        let once = dispatch(MergePolicy::Counters, Some(base), local, remote);
        // 合并后三侧一致：再并一遍不能变
        let again =
            crate::merge::merge(MergePolicy::Counters, Some(&once), Some(&once), Some(&once));
        assert_eq!(once, "微博\t23\n");
        assert_eq!(again, once);
    }

    #[test]
    fn deletion_propagates_as_a_negative_delta() {
        // 本机删了这个词（计数清零），远端基准内 10 → 全消
        let merged = crate::merge::merge(
            MergePolicy::Counters,
            Some("微博\t10\n"),
            Some(""),
            Some("微博\t10\n"),
        );
        assert_eq!(merged, "");
        // 远端又攒了 8：删掉的基准清零，新增的留下
        let merged = crate::merge::merge(
            MergePolicy::Counters,
            Some("微博\t10\n"),
            Some(""),
            Some("微博\t18\n"),
        );
        assert_eq!(merged, "微博\t8\n");
    }

    #[test]
    fn decay_propagates_as_a_negative_delta() {
        // 本机整体减半（10 → 5），远端还是 10 → 跟着减半
        let merged = dispatch(
            MergePolicy::Counters,
            Some("微博\t10\n"),
            "微博\t5\n",
            "微博\t10\n",
        );
        assert_eq!(merged, "微博\t5\n");
    }

    #[test]
    fn never_goes_below_zero() {
        // 远端比基准还少（它那台也删了）+ 本机没动：结果不为负
        let merged = dispatch(
            MergePolicy::Counters,
            Some("微博\t10\n"),
            "微博\t10\n",
            "微博\t3\n",
        );
        assert_eq!(merged, "微博\t3\n");
    }

    #[test]
    fn ngram_rows_of_both_shapes_merge_by_key() {
        // 二元 3 列与三元 4 列混在一张表里，各自成键；各自的增量独立算
        let merged = dispatch(
            MergePolicy::Counters,
            Some("<s>\t今天\t2\n今天\t天气\t1\n"),
            "<s>\t今天\t5\n",
            "今天\t天气\t4\n",
        );
        assert_eq!(merged, "<s>\t今天\t3\n今天\t天气\t3\n");
    }

    #[test]
    fn bad_lines_are_skipped() {
        let merged = dispatch(
            MergePolicy::Counters,
            Some("微博\t10\n坏行\n"),
            "微博\t12\n",
            "微博\t11\n微博二\n",
        );
        assert_eq!(merged, "微博\t13\n");
    }

    #[test]
    fn daily_stats_merge_every_column_independently() {
        let base = "2026-09-13\t100\t10\t1\t20\n";
        let local = "2026-09-13\t130\t10\t1\t25\n";
        let remote = "2026-09-13\t110\t15\t2\t20\n";
        let merged = dispatch(MergePolicy::DailyStats, Some(base), local, remote);
        assert_eq!(merged, "2026-09-13\t140\t15\t2\t25\n");
    }

    #[test]
    fn union_max_caps_at_the_fixed_frequency() {
        // 两台设备各自新增同一个词：100 上限，不翻倍
        let merged = dispatch(MergePolicy::UnionMax, None, "曼波\t100\n", "曼波\t100\n");
        assert_eq!(merged, "曼波\t100\n");
        // 删除也传播：本机删了，远端还是基准值 → 消失
        let merged = crate::merge::merge(
            MergePolicy::UnionMax,
            Some("曼波\t100\n"),
            Some(""),
            Some("曼波\t100\n"),
        );
        assert_eq!(merged, "");
    }
}
