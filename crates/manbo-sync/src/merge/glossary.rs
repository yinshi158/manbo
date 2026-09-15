//! 无计数的键值表三方合并：`user-glossary-<lang>.tsv`（`词\t[词性. ]译词[|读音]`）。
//!
//! 规则：本机改过的键（包括删掉）以本机为准——用户手改的释义优先于云端回填；
//! 本机没动的键跟随远端（另一台设备的回填与删除都收进来）。

use std::collections::BTreeMap;

use super::data_lines;

/// 解析一侧：词 → 行剩余部分原文（单个词条整段保留，含 `词性. ` 前缀与 `|读音`）。
fn parse(text: &str) -> BTreeMap<String, String> {
    let mut rows = BTreeMap::new();
    for line in data_lines(text) {
        match line.split_once('\t') {
            Some((word, rest)) => {
                rows.insert(word.to_owned(), rest.to_owned());
            }
            // 只有词没有释义的行也认，值记空串
            None => {
                rows.insert(line.to_owned(), String::new());
            }
        }
    }
    rows
}

pub(super) fn merge(base: &str, local: &str, remote: &str) -> String {
    let base = parse(base);
    let local = parse(local);
    let remote = parse(remote);
    let mut keys: std::collections::BTreeSet<&String> = base.keys().collect();
    keys.extend(local.keys());
    keys.extend(remote.keys());
    let mut out = String::new();
    for key in keys {
        // 本机相对基准动过这个键（改释义或删除）就听本机的；没动过就听远端的
        let winner = if local.get(key) != base.get(key) {
            local.get(key)
        } else {
            remote.get(key)
        };
        if let Some(value) = winner {
            out.push_str(key);
            if !value.is_empty() {
                out.push('\t');
                out.push_str(value);
            }
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_sides_new_words_are_kept() {
        let merged = merge("", "hello\thello. 你好\n", "gist\tn. 要旨\n");
        assert_eq!(merged, "gist\tn. 要旨\nhello\thello. 你好\n");
    }

    #[test]
    fn local_edits_win_over_remote() {
        let base = "gist\tn. 要旨\n";
        // 本机改了释义，远端还是基准 → 听本机
        let merged = merge(base, "gist\tn. 主旨；要点\n", base);
        assert_eq!(merged, "gist\tn. 主旨；要点\n");
        // 双方都改了 → 还是听本机（用户手改优先）
        let merged = merge(base, "gist\tn. 主旨\n", "gist\tn. 要点\n");
        assert_eq!(merged, "gist\tn. 主旨\n");
    }

    #[test]
    fn local_deletion_wins_but_remote_additions_arrive() {
        let base = "gist\tn. 要旨\nkeep\tv. 保持\n";
        // 本机删了 gist（相对基准动了），没动 keep，远端两个都有 → gist 消失、keep 留着
        let merged = merge(base, "keep\tv. 保持\n", base);
        assert_eq!(merged, "keep\tv. 保持\n");
    }

    #[test]
    fn untouched_keys_follow_remote_deletions() {
        let base = "gist\tn. 要旨\n";
        // 本机没动，远端删了 → 跟着删
        let merged = merge(base, base, "");
        assert_eq!(merged, "");
    }
}
