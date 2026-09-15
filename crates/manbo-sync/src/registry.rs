//! 同步文件登记表：哪些文件参与同步、每张用哪种合并策略、哪些能趁引擎运行时合并。
//!
//! **绝不上名单的**：`input-log.jsonl`（上屏原文 + 应用名，隐私红线）、`.env`（密钥）、
//! `dicts/`（可重新导入）、`model/`（大文件）、`logs/`、`sync/` 自身。

use std::path::Path;

/// 一轮同步的范围。启动 / 退出跑全量；运行中借落盘节拍跑学习表增量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncScope {
    /// 六张学习表 + config.toml：引擎在内存里也持有它们，合并完要 [`Engine::set_learner`] 热替换。
    ///
    /// [`Engine::set_learner`]: manbo_core::Engine::set_learner
    Learning,

    /// [`SyncScope::Learning`] 之外再加统计与释义文件。这些引擎也在内存里持有，但价值低，
    /// 只在引擎还没起（启动）或已停写（退出）时合并，重启自然收敛，不为它们加更多 Engine API。
    Full,
}

/// 一张表的合并策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePolicy {
    /// 「键 → 计数」表：`merged[键] = remote + (local − base)`，负增量即删除 / 衰减传播，零行丢弃。
    /// 键是除最后一列外的全部列（`user-ngram.tsv` 的 3 列 / 4 列两种行因此都成立）。
    Counters,

    /// 键是第一列、后面全是计数列：`usage.tsv`（日期 → 汉字 / 中文词 / 英文词 / 上屏次数）。
    /// 每列独立走 [`MergePolicy::Counters`] 的公式。
    DailyStats,

    /// 固定词频的并集（`user-words.tsv`，词频恒 100）：计数公式套上 100 的上限，
    /// 两台设备各自新增同一个词不会翻倍成 200。
    UnionMax,

    /// 词汇记录（`user-vocab.tsv`）：看到 / 上屏 / 用过三列按计数合并，首见取早、末见取晚。
    Vocabulary,

    /// 无计数的键值表（`user-glossary-<lang>.tsv`）：键 = 第一列，值 = 剩余列原文。
    /// 本地改过的键（含删除）以本地为准——用户手改的释义优先于云端回填。
    Glossary,

    /// 整文件三方（`config.toml`）：只有一方变了取那方；双方都变取 mtime 新的一方，输方落冲突副本。
    WholeFile,
}

/// 一个参与同步的文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFile {
    /// 数据目录下的文件名（本 crate 只同步数据目录顶层，无子路径）。
    pub name: String,

    /// 用哪种合并策略。
    pub policy: MergePolicy,

    /// 引擎运行时能否合并：学习表落盘后由壳 `set_learner` 热替换，配置由壳的配置热加载接管；
    /// 统计类引擎也在内存里持有，只有 false（启动 / 退出才合）。
    pub runtime: bool,
}

/// 学习表六个文件名。`user-words.tsv` 用 [`MergePolicy::UnionMax`]，其余按计数合并。
const LEARNING_TABLES: [(&str, MergePolicy); 6] = [
    ("user.tsv", MergePolicy::Counters),
    ("user-words.tsv", MergePolicy::UnionMax),
    ("user-ngram.tsv", MergePolicy::Counters),
    ("user-choices.tsv", MergePolicy::Counters),
    ("user-english.tsv", MergePolicy::Counters),
    ("user-typos.tsv", MergePolicy::Counters),
];

/// 个人释义表固定会试的语言码（壳的学习语言目前是 en / ja）；数据目录里还有别的语言文件也照收。
pub const GLOSSARY_LANGUAGES: [&str; 2] = ["en", "ja"];

/// 列出这一轮要同步的文件。`scope = Learning` 不碰磁盘；`Full` 扫一遍数据目录找现存释义表，
/// 再补上固定语言码（另一台设备传上来、本机还没生成的）。
pub fn resolve(data_dir: &Path, scope: SyncScope) -> Vec<SyncFile> {
    let mut files: Vec<SyncFile> = LEARNING_TABLES
        .into_iter()
        .map(|(name, policy)| SyncFile {
            name: name.to_owned(),
            policy,
            runtime: true,
        })
        .collect();
    files.push(SyncFile {
        name: "config.toml".to_owned(),
        policy: MergePolicy::WholeFile,
        runtime: true,
    });
    if scope == SyncScope::Full {
        files.push(SyncFile {
            name: "usage.tsv".to_owned(),
            policy: MergePolicy::DailyStats,
            runtime: false,
        });
        files.push(SyncFile {
            name: "user-vocab.tsv".to_owned(),
            policy: MergePolicy::Vocabulary,
            runtime: false,
        });
        for language in GLOSSARY_LANGUAGES {
            files.push(SyncFile {
                name: format!("user-glossary-{language}.tsv"),
                policy: MergePolicy::Glossary,
                runtime: false,
            });
        }
        let _ = scan_extra_glossaries(data_dir, &mut files);
    }
    files
}

/// 数据目录里已存在的其他语言释义表（`user-glossary-<码>.tsv`）也进名单；固定语言码已在表里的跳过。
fn scan_extra_glossaries(data_dir: &Path, files: &mut Vec<SyncFile>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(data_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(stem) = name.strip_prefix("user-glossary-") else {
            continue;
        };
        let Some(code) = stem.strip_suffix(".tsv") else {
            continue;
        };
        if GLOSSARY_LANGUAGES.contains(&code) {
            continue;
        }
        if files.iter().any(|file| file.name == name) {
            continue;
        }
        files.push(SyncFile {
            name: name.to_owned(),
            policy: MergePolicy::Glossary,
            runtime: false,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learning_scope_is_exactly_seven_files() {
        let files = resolve(Path::new("/nowhere"), SyncScope::Learning);
        assert_eq!(files.len(), 7);
        assert!(files.iter().all(|file| file.runtime));
        assert_eq!(files[0].name, "user.tsv");
        assert_eq!(files[1].policy, MergePolicy::UnionMax);
        assert!(files.iter().any(|file| file.name == "config.toml"));
    }

    #[test]
    fn full_scope_adds_stats_and_glossaries() {
        let dir = std::env::temp_dir().join(format!("manbo-sync-registry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("user-glossary-ja.tsv"), "みんな\teveryone\n").unwrap();
        std::fs::write(dir.join("user-glossary-fr.tsv"), "bonjour\n").unwrap();
        std::fs::write(dir.join("input-log.jsonl"), "{}\n").unwrap();
        let files = resolve(&dir, SyncScope::Full);
        let names: Vec<_> = files.iter().map(|file| file.name.as_str()).collect();
        assert!(names.contains(&"usage.tsv"));
        assert!(names.contains(&"user-vocab.tsv"));
        assert!(names.contains(&"user-glossary-ja.tsv"));
        assert!(names.contains(&"user-glossary-fr.tsv"), "扫描到的额外语言要进名单");
        assert!(!names.contains(&"input-log.jsonl"), "输入日志永不同步");
        let stats = files.iter().find(|file| file.name == "usage.tsv").unwrap();
        assert!(!stats.runtime, "统计文件只在启动 / 退出合并");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
