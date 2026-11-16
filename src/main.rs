mod activation;
mod aumid;
mod cdp;
mod config;

use anyhow::{Context, Result};
use config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let config = Config::from_env()?;

    if args.iter().any(|arg| arg == "--print-aumid") {
        println!("{}", aumid::discover(config.aumid.as_deref())?);
        return Ok(());
    }

    println!("Codex++ Launcher (Rust)");
    match config.cdp_port {
        Some(port) => println!("CDP 端口: {port}"),
        None => println!("远程调试: 已关闭（CODEX_PLUS_CDP_PORT=0）"),
    }
    if !config.cdp_args.is_empty() {
        println!("附加参数: {}", config.cdp_args.join(" "));
    }

    // 1. 先确定要启动哪个应用：显式 AUMID 优先，否则扫描已注册的 Store 包。
    let aumid = aumid::discover(config.aumid.as_deref())?;
    println!("使用 AUMID: {aumid}");

    // 2. 结束旧实例，避免单实例锁复用导致新的调试参数不生效。
    if config.kill_existing {
        println!("正在结束已有的 ChatGPT / Codex 进程...");
        activation::kill_existing_chatgpt().await?;
    }

    // 3. 通过 AUMID 激活应用；开启远程调试时传入调试参数，否则只传附加参数。
    let arguments = activation::build_activation_arguments(config.cdp_port, &config.cdp_args);
    if arguments.is_empty() {
        println!("正在激活应用（无附加参数）...");
    } else {
        println!("正在激活应用，参数: {arguments}");
    }
    let pid = activation::activate_packaged_app(&aumid, &arguments)
        .await
        .context("激活 Codex / ChatGPT 失败")?;
    println!("应用已启动，PID: {pid}");

    // 4. 关闭远程调试时到此为止；否则等 CDP 就绪后注入脚本。
    let Some(cdp_port) = config.cdp_port else {
        println!("远程调试已关闭，跳过脚本注入。");
        return Ok(());
    };

    println!(
        "等待 CDP 端点就绪（最多 {} 秒）...",
        config.cdp_timeout.as_secs()
    );
    let outcome = cdp::wait_and_inject(cdp_port, &config.inject_script, config.cdp_timeout)
        .await
        .context("等待 CDP 端点或注入脚本失败")?;

    println!(
        "注入完成: {} ({})",
        if outcome.title.is_empty() {
            "<无标题>"
        } else {
            &outcome.title
        },
        outcome.url
    );
    println!("Runtime.evaluate 结果: {}", outcome.result);

    Ok(())
}

fn print_help() {
    println!(
        r#"Codex++ Launcher

用法:
  codex-plus-launcher                 启动 Codex 并注入脚本
  codex-plus-launcher --print-aumid   只打印自动发现的 AUMID
  codex-plus-launcher --help          显示本帮助

环境变量:
  CODEX_PLUS_AUMID                 手动指定 AUMID（默认自动从已注册 Store 包中查找）
  CODEX_PLUS_CDP_PORT              远程调试端口，0 表示关闭调试与注入（默认 9229）
  CODEX_PLUS_CDP_ARGS              追加的 CDP 参数，支持引号（默认为空）
  CODEX_PLUS_INJECT_SCRIPT         注入脚本内容（默认一行 console.log）
  CODEX_PLUS_INJECT_SCRIPT_PATH    从文件读取注入脚本，优先于 CODEX_PLUS_INJECT_SCRIPT
  CODEX_PLUS_KILL_EXISTING         启动前结束旧实例，0/false/no/off 关闭（默认开启）
  CODEX_PLUS_CDP_TIMEOUT_SECS      等待 CDP 就绪的秒数（默认 60）
"#
    );
}
