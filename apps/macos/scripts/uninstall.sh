#!/bin/sh
# 卸载曼波：删掉输入法本体（系统级 /Library 与用户级 ~/Library 两处都看），加 --purge 连学习数据、配置、日志一起删。
# 这个脚本会打进 .app 的 Resources 里，装了 pkg 的用户直接运行它。
set -u
purge=0
[ "${1:-}" = "--purge" ] && purge=1

pkill -x manbo-macos 2>/dev/null || true

if [ -d "/Library/Input Methods/Manbo.app" ]; then
    echo "删除 /Library/Input Methods/Manbo.app（需要管理员密码）"
    sudo rm -rf "/Library/Input Methods/Manbo.app"
    sudo pkgutil --forget app.manbo.inputmethod >/dev/null 2>&1 || true
fi
if [ -d "$HOME/Library/Input Methods/Manbo.app" ]; then
    echo "删除 ~/Library/Input Methods/Manbo.app"
    rm -rf "$HOME/Library/Input Methods/Manbo.app"
fi

if [ "$purge" = 1 ]; then
    rm -rf "$HOME/Library/Application Support/Manbo" "$HOME/Library/Logs/Manbo"
    echo "学习数据、配置与日志已删除"
else
    echo "保留了 ~/Library/Application Support/Manbo（学习数据、配置）与 ~/Library/Logs/Manbo；要一起删请加 --purge"
fi
echo "已卸载。输入法列表里的「曼波」条目会在注销再登录后消失。"
