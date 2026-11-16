use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::time::Duration;

pub const DEFAULT_CDP_PORT: u16 = 9229;

/// 默认注入脚本。用 `CODEX_PLUS_INJECT_SCRIPT`（脚本内容）或
/// `CODEX_PLUS_INJECT_SCRIPT_PATH`（脚本文件）替换。
const DEFAULT_INJECT_SCRIPT: &str = r#"console.log('[CodexPlus] Renderer injected.');"#;

/// 启动器配置。全部来自环境变量，便于在不改代码的情况下调整。
#[derive(Debug, Clone)]
pub struct Config {
    /// 显式指定的 AUMID；为 `None` 时自动从已安装的 Store 包中查找。
    pub aumid: Option<String>,
    /// 远程调试端口；为 `None` 表示关闭远程调试和脚本注入。
    pub cdp_port: Option<u16>,
    /// 追加到 `--remote-debugging-port` 后面的额外参数。
    pub cdp_args: Vec<String>,
    pub inject_script: String,
    /// 启动前是否结束已有的 ChatGPT / Codex 进程。
    pub kill_existing: bool,
    /// 等待 CDP 端点就绪的总时长。
    pub cdp_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let aumid = non_empty_env("CODEX_PLUS_AUMID");
        let cdp_port = parse_cdp_port(non_empty_env("CODEX_PLUS_CDP_PORT"))?;

        let debug_enabled = cdp_port.is_some();

        // 默认不追加任何参数。
        let cdp_args = match env::var("CODEX_PLUS_CDP_ARGS") {
            Ok(raw) => split_args(&raw)?,
            Err(_) => Vec::new(),
        };

        // 关闭远程调试时，注入脚本和超时配置都不参与启动，读取失败也不应阻断。
        let inject_script = if debug_enabled {
            match non_empty_env("CODEX_PLUS_INJECT_SCRIPT_PATH") {
                Some(path) => fs::read_to_string(&path)
                    .with_context(|| format!("读取 CODEX_PLUS_INJECT_SCRIPT_PATH 失败: {path}"))?,
                None => env::var("CODEX_PLUS_INJECT_SCRIPT")
                    .unwrap_or_else(|_| DEFAULT_INJECT_SCRIPT.to_string()),
            }
        } else {
            String::new()
        };

        let kill_existing = match non_empty_env("CODEX_PLUS_KILL_EXISTING") {
            Some(value) => !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            ),
            None => true,
        };

        let cdp_timeout = if debug_enabled {
            match non_empty_env("CODEX_PLUS_CDP_TIMEOUT_SECS") {
                Some(value) => Duration::from_secs(value.parse::<u64>().with_context(|| {
                    format!("CODEX_PLUS_CDP_TIMEOUT_SECS 不是合法秒数: {value}")
                })?),
                None => Duration::from_secs(60),
            }
        } else {
            Duration::ZERO
        };

        Ok(Self {
            aumid,
            cdp_port,
            cdp_args,
            inject_script,
            kill_existing,
            cdp_timeout,
        })
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// `CODEX_PLUS_CDP_PORT` 为 `0` 时返回 `None`，表示关闭远程调试。
fn parse_cdp_port(value: Option<String>) -> Result<Option<u16>> {
    match value {
        Some(value) if value == "0" => Ok(None),
        Some(value) => {
            Ok(Some(value.parse::<u16>().with_context(|| {
                format!("CODEX_PLUS_CDP_PORT 不是合法端口: {value}")
            })?))
        }
        None => Ok(Some(DEFAULT_CDP_PORT)),
    }
}

/// 按 shell 的习惯拆分参数，支持单引号、双引号和反斜杠转义。
pub fn split_args(raw: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => {
                quote = None;
            }
            Some('"') if c == '\\' => match chars.peek().copied() {
                Some(next @ ('"' | '\\')) => {
                    current.push(next);
                    chars.next();
                }
                _ => current.push(c),
            },
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None => {
                current.push(c);
                started = true;
            }
        }
    }

    if let Some(q) = quote {
        bail!("CODEX_PLUS_CDP_ARGS 中的 {q} 引号没有闭合");
    }
    if started {
        args.push(current);
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::{parse_cdp_port, split_args};

    #[test]
    fn zero_port_disables_remote_debugging() {
        assert_eq!(parse_cdp_port(Some("0".into())).unwrap(), None);
        assert_eq!(parse_cdp_port(Some("9333".into())).unwrap(), Some(9333));
        assert_eq!(parse_cdp_port(None).unwrap(), Some(super::DEFAULT_CDP_PORT));
        assert!(parse_cdp_port(Some("abc".into())).is_err());
    }

    #[test]
    fn splits_plain_arguments() {
        assert_eq!(
            split_args("--a=1  --b=2").unwrap(),
            vec!["--a=1".to_string(), "--b=2".to_string()]
        );
    }

    #[test]
    fn keeps_quoted_values_together() {
        assert_eq!(
            split_args(r#"--name="hello world" --path='C:\a b'"#).unwrap(),
            vec![
                "--name=hello world".to_string(),
                "--path=C:\\a b".to_string()
            ]
        );
    }

    #[test]
    fn empty_input_means_no_extra_arguments() {
        assert!(split_args("   ").unwrap().is_empty());
    }
}
