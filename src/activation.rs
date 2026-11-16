use anyhow::Context;
use std::process::Command;
use std::time::Duration;
use tokio::time::timeout;

/// ActivateApplication 偶尔会一直不返回，给一个明确的上限。
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(60);

/// 通过 AUMID 激活打包应用（MSIX/UWP），并传递启动参数。
///
/// `arguments` 是完整的 Windows 命令行字符串，调用 [`build_activation_arguments`] 生成。
pub async fn activate_packaged_app(aumid: &str, arguments: &str) -> anyhow::Result<u32> {
    let aumid = aumid.to_string();
    let arguments = arguments.to_string();
    let task = tokio::task::spawn_blocking(move || activate_blocking(&aumid, &arguments));

    match timeout(ACTIVATION_TIMEOUT, task).await {
        Ok(result) => result.context("激活任务异常退出")?,
        Err(_) => anyhow::bail!(
            "ActivateApplication 超过 {} 秒仍未返回，Codex 可能没有正常启动",
            ACTIVATION_TIMEOUT.as_secs()
        ),
    }
}

#[cfg(windows)]
fn activate_blocking(aumid: &str, arguments: &str) -> anyhow::Result<u32> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, ACTIVATEOPTIONS,
    };

    const RPC_E_CHANGED_MODE: i32 = -2147417850;

    unsafe {
        let hresult = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let should_uninitialize = if hresult.is_ok() {
            true
        } else if hresult.0 == RPC_E_CHANGED_MODE {
            // 调用线程已经用别的模式初始化过 COM，可以直接继续。
            false
        } else {
            anyhow::bail!("初始化 COM 失败: 0x{:08X}", hresult.0);
        };

        let result = (|| -> anyhow::Result<u32> {
            let manager: IApplicationActivationManager =
                CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_ALL)
                    .context("无法创建 IApplicationActivationManager 实例")?;
            let pid = manager
                .ActivateApplication(
                    &HSTRING::from(aumid),
                    &HSTRING::from(arguments),
                    ACTIVATEOPTIONS(0),
                )
                .context("ActivateApplication 调用失败")?;
            Ok(pid)
        })();

        if should_uninitialize {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
fn activate_blocking(_aumid: &str, _arguments: &str) -> anyhow::Result<u32> {
    anyhow::bail!("当前平台不支持通过 AUMID 激活打包应用")
}

/// 拼出需要传给应用的完整命令行：调试端口 + 用户配置的额外参数。
///
/// `cdp_port` 为 `None` 时关闭远程调试，只保留额外参数。
pub fn build_activation_arguments(cdp_port: Option<u16>, extra_args: &[String]) -> String {
    let mut args = Vec::new();
    if let Some(cdp_port) = cdp_port {
        args.push(format!("--remote-debugging-port={cdp_port}"));
        args.push(format!(
            "--remote-allow-origins=http://127.0.0.1:{cdp_port}"
        ));
    }
    args.extend(extra_args.iter().cloned());
    args.iter()
        .map(|arg| quote_windows_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按 Windows 命令行的规则给参数加引号。
fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    if !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string();
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(c);
            }
        }
    }
    // 结尾的反斜杠必须翻倍，否则会把结束引号转义掉。
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// 结束正在运行的 Codex / ChatGPT 主进程，确保新参数生效。
///
/// 进程不存在时 taskkill 会返回非零状态码，这属于正常情况，不当作失败。
pub async fn kill_existing_chatgpt() -> anyhow::Result<()> {
    tokio::task::spawn_blocking(|| {
        Command::new("taskkill")
            .args(["/F", "/T", "/IM", "ChatGPT.exe"])
            .output()
            .context("执行 taskkill 失败")?;
        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("结束 ChatGPT 进程的任务异常退出")??;

    // 给旧实例一点时间释放单实例锁和调试端口。
    tokio::time::sleep(Duration::from_secs(1)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_activation_arguments, quote_windows_arg};

    #[test]
    fn quotes_arguments_with_spaces_and_quotes() {
        assert_eq!(quote_windows_arg("--plain"), "--plain");
        assert_eq!(
            quote_windows_arg("--name=hello world"),
            "\"--name=hello world\""
        );
        assert_eq!(quote_windows_arg(r#"--name=a"b"#), r#""--name=a\"b""#);
        assert_eq!(
            quote_windows_arg(r"C:\dir with space\"),
            r#""C:\dir with space\\""#
        );
    }

    #[test]
    fn building_arguments_keeps_debug_port_first() {
        let arguments = build_activation_arguments(
            Some(9333),
            &["--proxy-server=socks5://127.0.0.1:1080".into()],
        );
        assert_eq!(
            arguments,
            "--remote-debugging-port=9333 \
             --remote-allow-origins=http://127.0.0.1:9333 \
             --proxy-server=socks5://127.0.0.1:1080"
        );
    }

    #[test]
    fn no_extra_arguments_by_default() {
        assert_eq!(
            build_activation_arguments(Some(9229), &[]),
            "--remote-debugging-port=9229 --remote-allow-origins=http://127.0.0.1:9229"
        );
    }

    #[test]
    fn disabled_debug_port_only_passes_extra_arguments() {
        assert_eq!(build_activation_arguments(None, &[]), "");
        assert_eq!(
            build_activation_arguments(None, &["--lang=zh-CN".into()]),
            "--lang=zh-CN"
        );
    }
}
