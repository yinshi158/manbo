//! 「同步」页：`[sync]` 各项、「立即同步」按钮与上次同步状态。
//!
//! 「立即同步」只写一个触发文件（`<数据目录>/sync/request`），真正的合并由持有 Engine 的
//! Server 进程在其节拍上执行——设置进程直接改学习文件会跟引擎内存打架。

use manbo_sync::SyncState;
use windows_reactor::*;

use crate::panel::controls::{field, index_of, labeled, note, page};
use crate::panel::{Message, Settings};

/// 同步后端：界面名 + 配置写法。
pub(crate) const BACKENDS: [(&str, &str); 2] = [
    ("WebDAV（坚果云 / Nextcloud / NAS）", "webdav"),
    ("本地文件夹（iCloud / Dropbox / Syncthing）", "folder"),
];

fn string_combo(
    options: &'static [(&str, &str)],
    current: &str,
    callback: Callback<Option<usize>>,
) -> ComboBox {
    ComboBox::new()
        .items_source(options.iter().map(|(label, _)| *label))
        .selected_index(index_of(options, current))
        .on_selection_changed(callback)
}

/// 读 `<数据目录>/sync/state.json`；没有或坏掉都当「还没同步过」。
fn read_state(settings: &Settings) -> Option<SyncState> {
    let path = settings.data_dir().join("sync").join("state.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn view(settings: &Settings, context: &mut ViewContext<Settings>) -> View {
    let s = &settings.config.sync;
    let state = read_state(settings);
    let status = match state {
        Some(state) => match (&state.last_error, &state.last_success) {
            (Some(error), _) => format!("上次同步失败：{error}"),
            (None, Some(success)) => format!("上次同步：{success}"),
            (None, None) => "还没同步过".to_owned(),
        },
        None => "还没同步过".to_owned(),
    };
    let rows = [
        field(
            "启用同步",
            "把用户词、词频、个人 n-gram、选择记录、敲错表、英文词、打字统计、个人释义表与配置文件在多台设备之间合并同步；输入日志与密钥永不上传。",
            ToggleSwitch::new()
                .is_on(s.enabled)
                .on_toggled(context.callback(Message::SyncEnabled)),
        ),
        field(
            "后端",
            "",
            string_combo(&BACKENDS, s.backend.key(), context.callback(Message::SyncBackend)),
        ),
        field(
            "WebDAV 地址",
            "坚果云填 https://dav.jianguoyun.com/dav/manbo；后端选「本地文件夹」时不用。",
            TextBox::new()
                .text(s.url.clone())
                .on_text_changed(context.callback(Message::SyncUrl)),
        ),
        field(
            "用户名",
            "坚果云填登录邮箱。",
            TextBox::new()
                .text(s.username.clone())
                .on_text_changed(context.callback(Message::SyncUsername)),
        ),
        field(
            "密码",
            "坚果云用「应用密码」（网页端 账户信息 → 安全选项 里生成），只保存在本机。留空则读环境变量 MANBO_SYNC_PASSWORD。",
            PasswordBox::new()
                .password(s.password.clone().unwrap_or_default())
                .placeholder_text("留空则读环境变量 MANBO_SYNC_PASSWORD")
                .on_password_changed(context.callback(Message::SyncPassword)),
        ),
        field(
            "同步文件夹",
            "后端选「本地文件夹」时用：输入法把合并后的数据写进这个目录，交给 iCloud Drive / Dropbox / Syncthing 传输。",
            TextBox::new()
                .text(s.folder.as_ref().map(|path| path.to_string_lossy().into_owned()).unwrap_or_default())
                .on_text_changed(context.callback(Message::SyncFolder)),
        ),
        field(
            "自动同步间隔（分钟）",
            "运行中每这么久自动同步一次；启动与退出各固定同步一轮。",
            NumberBox::new()
                .minimum(1.0)
                .maximum(1440.0)
                .value(s.interval_minutes as f64)
                .on_value_changed(context.callback(Message::SyncInterval)),
        ),
        labeled(
            "",
            StackPanel::new()
                .orientation(Orientation::Horizontal)
                .spacing(12.0)
                .children((
                    Button::new()
                        .on_click(context.message(Message::SyncNow))
                        .content("立即同步"),
                    TextBlock::new().text(status),
                )),
        ),
        note(
            "立即同步由输入法进程在后台执行，完成后再看这里的状态。两台设备各自攒的记录会合并而不是互相覆盖。",
        ),
    ];
    page("同步", StackPanel::new().spacing(16.0).children(rows))
}
