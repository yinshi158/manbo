//! 双设备集成测试：两个临时目录当两台设备，第三个目录当「云」（folder 后端），
//! 交替跑全量同步，验证收敛、删除传播、幂等与配置冲突副本。

use std::fs;
use std::path::{Path, PathBuf};

use manbo_sync::{run_once, BackendKind, SyncConfig, SyncScope};

/// 每个测试一个独立目录。
fn scratch(test: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("manbo-sync-two-{test}-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// 指向「云」目录的 folder 配置。
fn config_for(cloud: &Path) -> SyncConfig {
    SyncConfig {
        enabled: true,
        backend: BackendKind::Folder,
        folder: Some(cloud.to_owned()),
        startup_timeout_ms: 10_000,
        ..SyncConfig::default()
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn two_devices_converge_and_both_inputs_survive() {
    let cloud = scratch("converge", "cloud");
    let device_a = scratch("converge", "a");
    let device_b = scratch("converge", "b");

    // A 有词频，首同步推上云
    fs::write(device_a.join("user.tsv"), "微博\t10\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&cloud.join("user.tsv")), "微博\t10\n");

    // B 另有一份，同步后云上是并集
    fs::write(device_b.join("user.tsv"), "知乎\t4\n").unwrap();
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&cloud.join("user.tsv")), "微博\t10\n知乎\t4\n");

    // A 再同步，拿到 B 的词；此后三方一致
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_a.join("user.tsv")), "微博\t10\n知乎\t4\n");

    // 幂等：谁再跑一遍都不变
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&cloud.join("user.tsv")), "微博\t10\n知乎\t4\n");
    assert_eq!(read(&device_b.join("user.tsv")), "微博\t10\n知乎\t4\n");
}

#[test]
fn independent_devices_counts_sum_on_first_contact() {
    let cloud = scratch("count", "cloud");
    let device_a = scratch("count", "a");
    let device_b = scratch("count", "b");
    // 两台设备各自攒了 5 次「你好」，互不知情，先后首同步：真实选择次数就是 10，求和正确
    // （重复计数只会发生在「只拷 TSV 到新设备再开同步」的迁移方式里；整目录拷贝连 sync/ 基准
    // 一起带走不会重复，用户文档里写明推荐前者）
    fs::write(device_a.join("user.tsv"), "你好\t5\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    fs::write(device_b.join("user.tsv"), "你好\t5\n").unwrap();
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&cloud.join("user.tsv")), "你好\t10\n");
    // A 下一轮拿到 B 的存在，总数收敛不再变
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_a.join("user.tsv")), "你好\t10\n");
}

#[test]
fn deletion_propagates_to_the_other_device() {
    let cloud = scratch("delete", "cloud");
    let device_a = scratch("delete", "a");
    let device_b = scratch("delete", "b");
    fs::write(device_a.join("user.tsv"), "微博\t10\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    // B 拉下来
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_b.join("user.tsv")), "微博\t10\n");
    // A 删掉这个词（用户删候选 / 清了表），同步后云端清空
    fs::write(device_a.join("user.tsv"), "").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&cloud.join("user.tsv")), "");
    // B 下一轮跟着清掉
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_b.join("user.tsv")), "");
}

#[test]
fn learned_words_and_typo_tables_travel_together() {
    let cloud = scratch("tables", "cloud");
    let device_a = scratch("tables", "a");
    let device_b = scratch("tables", "b");
    fs::write(device_a.join("user-words.tsv"), "曼波\tmanbo\t100\n").unwrap();
    fs::write(device_a.join("user-typos.tsv"), "zang\tzhang\t3\n").unwrap();
    fs::write(device_a.join("user-english.tsv"), "gist\t2\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_b.join("user-words.tsv")), "曼波\tmanbo\t100\n");
    assert_eq!(read(&device_b.join("user-typos.tsv")), "zang\tzhang\t3\n");
    assert_eq!(read(&device_b.join("user-english.tsv")), "gist\t2\n");
}

#[test]
fn glossaries_sync_between_devices() {
    let cloud = scratch("glossary", "cloud");
    let device_a = scratch("glossary", "a");
    let device_b = scratch("glossary", "b");
    fs::write(device_a.join("user-glossary-en.tsv"), "gist\tn. 要旨\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_b.join("user-glossary-en.tsv")), "gist\tn. 要旨\n");
}

#[test]
fn config_both_changed_keeps_a_conflict_copy_and_picks_the_newer() {
    let cloud = scratch("config", "cloud");
    let device_a = scratch("config", "a");
    let device_b = scratch("config", "b");
    // 基准：A 先把一份配置推上云
    fs::write(device_a.join("config.toml"), "[general]\npage_size = 9\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    // B 拉下来，本地改成 page_size = 5
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    fs::write(device_b.join("config.toml"), "[general]\npage_size = 5\n").unwrap();
    // A 改成 theme = dark 并推上云（云端 mtime 晚于 B 的本地修改）
    fs::write(device_a.join("config.toml"), "[general]\ntheme = \"dark\"\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    // B 再同步：双方都改过 → 云端的更新，B 的本地修改留冲突副本，不丢
    run_once(&config_for(&cloud), &device_b, SyncScope::Full, 10_000).unwrap();
    assert_eq!(read(&device_b.join("config.toml")), "[general]\ntheme = \"dark\"\n");
    let copies: Vec<_> = fs::read_dir(&device_b)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("config.conflict-"))
        .collect();
    assert_eq!(copies.len(), 1, "应有一个冲突副本：{copies:?}");
    assert_eq!(read(&device_b.join(&copies[0])), "[general]\npage_size = 5\n");
}

#[test]
fn input_log_is_never_synced() {
    let cloud = scratch("privacy", "cloud");
    let device_a = scratch("privacy", "a");
    fs::write(device_a.join("input-log.jsonl"), "{\"text\":\"绝密\"}\n").unwrap();
    fs::write(device_a.join(".env"), "MANBO_API_KEY=secret\n").unwrap();
    run_once(&config_for(&cloud), &device_a, SyncScope::Full, 10_000).unwrap();
    assert!(!cloud.join("input-log.jsonl").exists(), "输入日志永不上传");
    assert!(!cloud.join(".env").exists(), "密钥永不上传");
}
