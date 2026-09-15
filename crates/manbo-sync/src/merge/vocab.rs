//! 词汇记录三方合并：`user-vocab.tsv`（`语言\t译词\t看到轮次\t上屏次数\t用过次数\t首见\t末见`）。
//!
//! 看到 / 上屏 / 用过三个计数列走计数公式；首见取两侧更早的、末见取两侧更晚的
//! （min / max 单调，不需要基准参与）。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::data_lines;

/// 一条词汇记录（首见 / 末见是 ISO 日期字符串，字典序即时间序）。
#[derive(Debug, Clone, Default)]
struct Entry {
    /// 上屏时在候选窗口里看到的轮次。
    seen: u64,

    /// 上屏过带它的候选几次。
    committed: u64,

    /// 直接打出过几次。
    used: u64,

    /// 第一次记录的日期。
    first: String,

    /// 最近一次记录的日期。
    last: String,
}

/// 键 = (语言, 译词)。
type Rows = BTreeMap<(String, String), Entry>;

fn parse(text: &str) -> Rows {
    let mut rows = Rows::new();
    for line in data_lines(text) {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 7 {
            continue;
        }
        let Ok(seen) = fields[2].trim().parse::<u64>() else {
            continue;
        };
        let Ok(committed) = fields[3].trim().parse::<u64>() else {
            continue;
        };
        let Ok(used) = fields[4].trim().parse::<u64>() else {
            continue;
        };
        let key = (fields[0].to_owned(), fields[1].to_owned());
        let entry = rows.entry(key).or_default();
        entry.seen += seen;
        entry.committed += committed;
        entry.used += used;
        entry.first = fields[5].to_owned();
        entry.last = fields[6].to_owned();
    }
    rows
}

pub(super) fn merge(base: &str, local: &str, remote: &str) -> String {
    let base = parse(base);
    let local = parse(local);
    let remote = parse(remote);
    let mut keys: std::collections::BTreeSet<&(String, String)> = base.keys().collect();
    keys.extend(local.keys());
    keys.extend(remote.keys());
    let mut out = String::new();
    for key in keys {
        let count = |rows: &Rows, column: usize| -> i64 {
            let value = match column {
                0 => rows.get(key).map(|entry| entry.seen),
                1 => rows.get(key).map(|entry| entry.committed),
                _ => rows.get(key).map(|entry| entry.used),
            };
            value.unwrap_or(0) as i64
        };
        let mut values = [0i64; 3];
        for (column, value) in values.iter_mut().enumerate() {
            let delta = count(&local, column) - count(&base, column);
            *value = (count(&remote, column) + delta).max(0);
        }
        if values == [0, 0, 0] {
            continue;
        }
        // 两侧都没有日期（另一侧是空表）时取现有的
        let first = [local.get(key), remote.get(key)]
            .into_iter()
            .flatten()
            .map(|entry| entry.first.as_str())
            .min()
            .unwrap_or_default();
        let last = [local.get(key), remote.get(key)]
            .into_iter()
            .flatten()
            .map(|entry| entry.last.as_str())
            .max()
            .unwrap_or_default();
        let (language, word) = key;
        let _ = writeln!(
            out,
            "{language}\t{word}\t{}\t{}\t{}\t{first}\t{last}",
            values[0], values[1], values[2]
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_sum_and_dates_take_min_max() {
        let base = "en\tgist\t2\t1\t0\t2026-09-01\t2026-09-05\n";
        let local = "en\tgist\t5\t2\t1\t2026-09-01\t2026-09-10\n";
        let remote = "en\tgist\t3\t1\t0\t2026-08-30\t2026-09-08\n";
        let merged = merge(base, local, remote);
        assert_eq!(merged, "en\tgist\t6\t2\t1\t2026-08-30\t2026-09-10\n");
    }

    #[test]
    fn first_sync_unions_both_sides() {
        let merged = merge(
            "",
            "en\tgist\t1\t0\t0\t2026-09-01\t2026-09-01\n",
            "ja\tgist\t2\t1\t0\t2026-09-02\t2026-09-03\n",
        );
        assert_eq!(
            merged,
            "en\tgist\t1\t0\t0\t2026-09-01\t2026-09-01\nja\tgist\t2\t1\t0\t2026-09-02\t2026-09-03\n"
        );
    }

    #[test]
    fn bad_lines_are_skipped() {
        let merged = merge(
            "",
            "en\tgist\t1\t0\t0\t2026-09-01\t2026-09-01\n坏行\nen\t短\t1\t0\n",
            "",
        );
        assert_eq!(merged, "en\tgist\t1\t0\t0\t2026-09-01\t2026-09-01\n");
    }
}
