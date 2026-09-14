const MESSAGES: &[(&str, &str)] = &[
    ("Ready", "就绪"),
    ("Server already running", "服务已在运行"),
    ("Server stopped", "服务已停止"),
    ("Server is not running", "服务尚未运行"),
    ("Checking dependencies...", "正在检查运行环境..."),
    (
        "Installing dependencies... (this may take a few minutes)",
        "正在安装依赖，可能需要几分钟...",
    ),
    (
        "No dependencies needed (server embedded).",
        "无需安装依赖，服务端已内嵌。",
    ),
    ("Starting Copilot auth flow...", "正在启动 Copilot 授权..."),
    ("Waiting for GitHub authorization", "正在等待 GitHub 授权"),
    (
        "GitHub authorized. Server restart required for account changes.",
        "GitHub 授权成功。更换账户后需重启服务。",
    ),
    (
        "Device code received. Open login URL to authorize.",
        "已收到设备验证码，请打开授权地址完成登录。",
    ),
    (
        "Device code copied to clipboard",
        "设备验证码已复制到剪贴板",
    ),
    ("Device code is empty", "设备验证码为空"),
    ("Login URL copied to clipboard", "授权地址已复制到剪贴板"),
    ("Login URL is empty", "授权地址为空"),
    ("Log copied to clipboard", "日志已复制到剪贴板"),
    ("Log is empty", "日志为空"),
    ("Log cleared", "日志已清空"),
    ("Hooks config opened", "Hooks 配置已打开"),
    ("Model list refreshed from server", "已从服务端刷新模型列表"),
    ("Autostart enabled", "已启用开机启动"),
    ("Autostart disabled", "已关闭开机启动"),
    ("Azure OpenAI disabled", "Azure OpenAI 已禁用"),
    ("Azure OpenAI config updated", "Azure OpenAI 配置已更新"),
    (
        "Claude Code gateway settings updated",
        "Claude Code 网关配置已更新",
    ),
    ("Invalid provider selection", "服务提供商选择无效"),
    (
        "Port must be between 1 and 65535",
        "端口必须在 1 到 65535 之间",
    ),
    ("Invalid account type", "账户类型无效"),
    (
        "Rate limit must not exceed 86400 seconds",
        "请求间隔不能超过 86400 秒",
    ),
    ("Invalid proxy URL", "代理地址无效"),
    (
        "Manual approval requires the interactive server CLI",
        "手动审批需要交互式服务端命令行",
    ),
    (
        "Invalid or incomplete Azure configuration",
        "Azure 配置无效或不完整",
    ),
    ("Invalid upstream API key", "上游 API 密钥无效"),
    ("Invalid GitHub token", "GitHub Token 无效"),
    ("Invalid upstream base URL", "上游 API 基础地址无效"),
    (
        "GUI configuration must be a JSON object",
        "GUI 配置必须是 JSON 对象",
    ),
    ("[OK] Ready to use", "[OK] 可以使用"),
    (
        "Build server or place it beside the GUI executable",
        "请构建服务端，或将其放在 GUI 程序旁",
    ),
    (
        "VS Code: [X] Missing (optional)",
        "VS Code: [X] 未安装（可选）",
    ),
    ("Extensions: [OK]", "扩展：[OK]"),
    ("Extensions: [-] Skipped", "扩展：[-] 已跳过"),
    (
        "Claude CLI: [X] Missing (optional, for Claude Code)",
        "Claude CLI: [X] 未安装（可选，供 Claude Code 使用）",
    ),
    (
        "Copilot API Server: [OK] Embedded",
        "Copilot API 服务：[OK] 已内嵌",
    ),
    (
        "Copilot API Server: External executable required",
        "Copilot API 服务：需要外部可执行文件",
    ),
];

const PREFIXES: &[(&str, &str)] = &[
    (
        "Saved. Azure OpenAI check failed: ",
        "已保存。Azure OpenAI 检查失败：",
    ),
    ("Saved. ", "已保存。"),
    ("Save failed: ", "保存失败："),
    ("Language preference save failed: ", "语言偏好保存失败："),
    ("Claude Code settings failed: ", "Claude Code 配置失败："),
    ("Azure OpenAI check failed: ", "Azure OpenAI 检查失败："),
    ("Autostart update failed: ", "开机启动设置失败："),
    ("Server process started on port ", "服务进程已启动，端口 "),
    ("Server start failed: ", "服务启动失败："),
    ("Server exited: ", "服务已退出："),
    (
        "Cannot open GitHub authorization URL: ",
        "无法打开 GitHub 授权地址：",
    ),
    ("Auth error: ", "授权失败："),
    ("Clipboard error: ", "剪贴板错误："),
    ("Open hooks config failed: ", "打开 Hooks 配置失败："),
    ("Model cache update failed: ", "模型缓存更新失败："),
];

fn known_message(message: &str) -> Option<&'static str> {
    MESSAGES
        .iter()
        .find_map(|(english, chinese)| (*english == message).then_some(*chinese))
}

pub fn translate_message(message: &str) -> String {
    if let Some(translated) = known_message(message) {
        return translated.to_string();
    }
    if let Some(extensions) = message
        .strip_prefix("Extensions: [X] Missing ")
        .and_then(|value| value.strip_suffix(" (optional)"))
    {
        return format!("扩展：[X] 缺少 {extensions}（可选）");
    }
    for (english, chinese) in PREFIXES {
        if let Some(detail) = message.strip_prefix(english) {
            return format!("{chinese}{}", known_message(detail).unwrap_or(detail));
        }
    }
    message.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_statuses_translate_without_changing_details() {
        assert_eq!(translate_message("Log cleared"), "日志已清空");
        assert_eq!(
            translate_message("Server process started on port 5050"),
            "服务进程已启动，端口 5050"
        );
        assert_eq!(
            translate_message("Saved. Azure OpenAI disabled"),
            "已保存。Azure OpenAI 已禁用"
        );
        assert_eq!(
            translate_message("Save failed: Port must be between 1 and 65535"),
            "保存失败：端口必须在 1 到 65535 之间"
        );
        assert_eq!(
            translate_message("Clipboard error: fixture OS failure"),
            "剪贴板错误：fixture OS failure"
        );
        assert_eq!(
            translate_message("upstream diagnostic https://localhost:5050/path"),
            "upstream diagnostic https://localhost:5050/path"
        );
    }

    #[test]
    fn dependency_messages_keep_tool_ids_and_versions() {
        assert_eq!(translate_message("[OK] Ready to use"), "[OK] 可以使用");
        assert_eq!(
            translate_message("Copilot API Server: [OK] Embedded"),
            "Copilot API 服务：[OK] 已内嵌"
        );
        assert_eq!(
            translate_message("VS Code: [OK] 1.104.3"),
            "VS Code: [OK] 1.104.3"
        );
        assert_eq!(
            translate_message("Extensions: [X] Missing github.copilot-chat (optional)"),
            "扩展：[X] 缺少 github.copilot-chat（可选）"
        );
    }
}
