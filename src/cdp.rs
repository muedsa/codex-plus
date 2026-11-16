use anyhow::{bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

/// /json/list 返回的 CDP target。
#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: String,
    #[serde(default, rename = "webSocketDebuggerUrl")]
    pub websocket_url: Option<String>,
}

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug, Clone)]
pub struct InjectionOutcome {
    pub title: String,
    pub url: String,
    /// Runtime.evaluate 返回的 result（脚本没有返回值时通常为空）。
    pub result: Value,
}

/// 等待 CDP 页面 target 出现，然后注入脚本。
///
/// 脚本同时通过 `Page.addScriptToEvaluateOnNewDocument` 注册到后续导航，
/// 并对当前页面立即执行一次。
pub async fn wait_and_inject(
    cdp_port: u16,
    script: &str,
    timeout: Duration,
) -> Result<InjectionOutcome> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .context("创建 CDP HTTP 客户端失败")?;

    let deadline = Instant::now() + timeout;

    loop {
        let error = match find_page_target(&client, cdp_port).await {
            Ok(Some(target)) => match inject(&target, script).await {
                Ok(result) => {
                    return Ok(InjectionOutcome {
                        title: target.title,
                        url: target.url,
                        result,
                    });
                }
                Err(error) => error,
            },
            Ok(None) => {
                anyhow::anyhow!("http://127.0.0.1:{cdp_port}/json/list 中还没有可注入的页面")
            }
            Err(error) => error,
        };

        if Instant::now() >= deadline {
            bail!("等待 CDP 端点超时（{} 秒）: {error:#}", timeout.as_secs());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

async fn find_page_target(client: &reqwest::Client, cdp_port: u16) -> Result<Option<Target>> {
    let url = format!("http://127.0.0.1:{cdp_port}/json/list");
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("请求 {url} 失败"))?;
    if !response.status().is_success() {
        bail!("{url} 返回 HTTP {}", response.status());
    }
    let targets: Vec<Target> = response
        .json()
        .await
        .with_context(|| format!("解析 {url} 的响应失败"))?;
    Ok(targets
        .into_iter()
        .filter(|target| target.websocket_url.is_some())
        .filter_map(|target| target_rank(&target).map(|rank| (rank, target)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, target)| target))
}

/// 数字越小越优先：主窗口（app://-/index.html）> 其他 app 页面 > 标题含 Codex/ChatGPT 的页面。
///
/// 启动器可能有多个 `app://-/` 页面（头像浮层、快捷聊天、独立窗口等），
/// 这里只注入主窗口，避免把脚本打进辅助页面。
fn target_rank(target: &Target) -> Option<u8> {
    if target.kind != "page" {
        return None;
    }
    let url = target.url.trim().to_ascii_lowercase();
    let title = target.title.trim().to_ascii_lowercase();
    if url.starts_with("devtools://") || url.starts_with("chrome-devtools://") {
        return None;
    }
    if is_auxiliary_app_page(&url) {
        return None;
    }
    if url == "app://-/index.html" {
        Some(0)
    } else if url.starts_with("app://-/") {
        Some(1)
    } else if title.contains("codex") || title.contains("chatgpt") {
        Some(2)
    } else {
        Some(3)
    }
}

fn is_auxiliary_app_page(url: &str) -> bool {
    url.contains("avatar-overlay") || url.contains("quick-chat") || url.contains("detached-window")
}

async fn inject(target: &Target, script: &str) -> Result<Value> {
    let websocket_url = target
        .websocket_url
        .as_deref()
        .context("目标没有 webSocketDebuggerUrl")?;

    let (mut ws, _) = connect_async(websocket_url)
        .await
        .with_context(|| format!("连接 CDP WebSocket 失败: {websocket_url}"))?;

    send_and_wait(&mut ws, 1, "Page.enable", json!({})).await?;
    send_and_wait(
        &mut ws,
        2,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({ "source": script }),
    )
    .await?;

    let result = send_and_wait(
        &mut ws,
        3,
        "Runtime.evaluate",
        json!({
            "expression": script,
            "awaitPromise": false,
            "returnByValue": false,
        }),
    )
    .await?;

    Ok(result.get("result").cloned().unwrap_or(Value::Null))
}

async fn send_and_wait(ws: &mut Ws, id: u64, method: &str, params: Value) -> Result<Value> {
    let payload = serde_json::to_string(&json!({
        "id": id,
        "method": method,
        "params": params,
    }))
    .context("序列化 CDP 命令失败")?;

    ws.send(Message::Text(payload.into()))
        .await
        .with_context(|| format!("发送 CDP 命令 {method} 失败"))?;

    loop {
        let message = ws
            .next()
            .await
            .with_context(|| format!("等待 CDP 命令 {method} 的结果时连接关闭"))??;

        match message {
            Message::Text(text) => {
                let value: Value = serde_json::from_str(text.as_str())
                    .with_context(|| format!("解析 CDP 命令 {method} 的结果失败"))?;
                if value.get("id").and_then(Value::as_u64) != Some(id) {
                    // 事件或其他命令的响应，继续等。
                    continue;
                }
                if let Some(error) = value.get("error") {
                    bail!("CDP 命令 {method} 返回错误: {error}");
                }
                return Ok(value.get("result").cloned().unwrap_or(Value::Null));
            }
            Message::Ping(payload) => {
                ws.send(Message::Pong(payload))
                    .await
                    .context("回复 CDP WebSocket Ping 失败")?;
            }
            Message::Close(frame) => {
                bail!("CDP WebSocket 在等待 {method} 结果时关闭: {frame:?}");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{target_rank, Target};

    fn target(kind: &str, title: &str, url: &str) -> Target {
        Target {
            kind: kind.into(),
            title: title.into(),
            url: url.into(),
            websocket_url: Some("ws://127.0.0.1:9229/devtools/page/1".into()),
        }
    }

    #[test]
    fn prefers_the_codex_main_page() {
        assert_eq!(
            target_rank(&target("page", "ChatGPT", "app://-/index.html")),
            Some(0)
        );
        assert_eq!(
            target_rank(&target(
                "page",
                "ChatGPT",
                "app://-/index.html?initialRoute=%2Fchatgpt%2Fquick-chat"
            )),
            None
        );
        assert_eq!(
            target_rank(&target(
                "page",
                "ChatGPT",
                "app://-/index.html?initialRoute=%2Fsettings"
            )),
            Some(1)
        );
        assert_eq!(
            target_rank(&target("page", "New Tab", "https://example.com")),
            Some(3)
        );
        assert_eq!(
            target_rank(&target("service_worker", "x", "app://-/sw.js")),
            None
        );
    }
}
