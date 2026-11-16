# Codex++ Launcher

通过 AUMID 启动 Microsoft Store 版 Codex（ChatGPT 桌面端），可以附加启动参数、开启 CDP 调试端口、注入脚本。

## 下载

可以在 [Action](https://github.com/muedsa/codex-plus/actions/workflows/build.yml) 下载最新构建。

## 流程

1. 读取配置（环境变量）。
2. 自动查找已安装的 Codex AUMID（也可手动指定）。
3. 结束已有的 `ChatGPT.exe`，避免单实例复用导致新的调试参数不生效。
4. 通过 `IApplicationActivationManager::ActivateApplication` 激活打包应用：
   - 默认传入 `--remote-debugging-port=<调试端口>` 和 `--remote-allow-origins=http://127.0.0.1:<调试端口>`；
   - 同时追加 `CODEX_PLUS_CDP_ARGS` 中的额外参数（默认为空）；
   - 当 `CODEX_PLUS_CDP_PORT=0` 时关闭远程调试，只追加额外参数。
5. 开启远程调试时，轮询 `http://127.0.0.1:<调试端口>/json/list`，找到页面 target。
6. 通过 CDP WebSocket：
   - `Page.addScriptToEvaluateOnNewDocument` 注册脚本；
   - `Runtime.evaluate` 在当前页面立即执行一次。
   
   关闭远程调试时跳过第 5、6 步，也不会注入脚本。

## 构建与运行

```powershell
cargo build --release
.\target\release\codex-plus-launcher.exe
```

只查看自动发现的 AUMID：

```powershell
cargo run -- --print-aumid
```

## 配置

所有配置都通过环境变量提供，修改后重新运行即可，无需改代码。

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `CODEX_PLUS_AUMID` | 自动查找 | 手动指定 AUMID，例如 `OpenAI.Codex_2p2nqsd0c76g0!App` |
| `CODEX_PLUS_CDP_PORT` | `9229` | 远程调试端口；设为 `0` 关闭远程调试和脚本注入 |
| `CODEX_PLUS_CDP_ARGS` | 空 | 追加到调试参数后的额外参数，支持引号 |
| `CODEX_PLUS_INJECT_SCRIPT` | 一行 `console.log` | 注入脚本内容 |
| `CODEX_PLUS_INJECT_SCRIPT_PATH` | 无 | 从文件读取注入脚本，优先于 `CODEX_PLUS_INJECT_SCRIPT` |
| `CODEX_PLUS_KILL_EXISTING` | 开启 | `0`/`false`/`no`/`off` 可跳过结束旧进程 |
| `CODEX_PLUS_CDP_TIMEOUT_SECS` | `60` | 等待 CDP 就绪的秒数 |

示例：

```powershell
$env:CODEX_PLUS_CDP_ARGS = '--proxy-server=socks5://127.0.0.1:1080 --lang=zh-CN'
$env:CODEX_PLUS_INJECT_SCRIPT_PATH = '.\inject.js'
.\target\release\codex-plus-launcher.exe
```

只带附加参数启动、不开启远程调试：

```powershell
$env:CODEX_PLUS_CDP_PORT = '0'
$env:CODEX_PLUS_CDP_ARGS = '--lang=zh-CN'
.\target\release\codex-plus-launcher.exe
```

`inject.js` 示例：

```js
console.log('[CodexPlus] Renderer injected.');
window.__codexPlusInjected = true;
```

## AUMID 自动发现

扫描当前用户已注册的 MSIX 包：

```
HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages
```

包键名是 `PackageFullName`（如 `OpenAI.Codex_26.1111.2228.0_x64__2p2nqsd0c76g0`），
其下代表可启动应用的子键（通常是 `App`），二者拼接即为 AUMID：

```
OpenAI.Codex_2p2nqsd0c76g0!App
```

优先选择包名以 `OpenAI.Codex` 开头的包，其次是显示名包含 `Codex` / `ChatGPT` 的包；
如果自动发现失败，会给出提示并可通过 `CODEX_PLUS_AUMID` 手动指定。

## License

[MIT](LICENSE)

