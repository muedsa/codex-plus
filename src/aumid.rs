use anyhow::Result;

/// 显式传入的 AUMID 优先；否则扫描当前用户已注册的 MSIX 包自动查找。
pub fn discover(explicit: Option<&str>) -> Result<String> {
    if let Some(explicit) = explicit {
        return validate(explicit);
    }

    #[cfg(windows)]
    {
        windows::find_installed_codex_aumid()
    }

    #[cfg(not(windows))]
    {
        anyhow::bail!("非 Windows 平台无法自动查找 AUMID，请通过 CODEX_PLUS_AUMID 指定")
    }
}

fn validate(aumid: &str) -> Result<String> {
    let aumid = aumid.trim();
    if aumid.is_empty() || !aumid.contains('!') {
        anyhow::bail!("CODEX_PLUS_AUMID 格式不正确，应形如 OpenAI.Codex_2p2nqsd0c76g0!App");
    }
    Ok(aumid.to_string())
}

#[cfg(windows)]
mod windows {
    use anyhow::{Context, Result};
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// 当前用户已注册的包列表。子键名就是 PackageFullName，包键下每个子键是一个
    /// Application Id，二者拼起来就是 AUMID。
    const PACKAGES_KEY: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

    pub(super) fn find_installed_codex_aumid() -> Result<String> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let packages = hkcu
            .open_subkey(PACKAGES_KEY)
            .with_context(|| format!("无法读取已安装应用列表 HKCU\\{PACKAGES_KEY}"))?;

        let mut best: Option<(u32, String)> = None;

        for package_name in packages.enum_keys().flatten() {
            let Ok(package) = packages.open_subkey(&package_name) else {
                continue;
            };
            let display_name: String = package.get_value("DisplayName").unwrap_or_default();
            let Some(base_score) = package_score(&package_name, &display_name) else {
                continue;
            };
            let Some(family_name) = package_family_name(&package_name) else {
                continue;
            };

            let app_ids: Vec<String> = package.enum_keys().flatten().collect();
            // 应用列表里真正可启动的是 "App"；其他 Application 多为后台辅助进程。
            let candidates: Vec<&String> = if app_ids.iter().any(|id| id == "App") {
                app_ids.iter().filter(|id| id.as_str() == "App").collect()
            } else {
                app_ids.iter().collect()
            };

            for app_id in candidates {
                if app_id.is_empty() || app_id.contains('!') {
                    continue;
                }
                let score = base_score + if app_id == "App" { 1000 } else { 0 };
                let aumid = format!("{family_name}!{app_id}");
                if best
                    .as_ref()
                    .is_none_or(|(best_score, _)| score > *best_score)
                {
                    best = Some((score, aumid));
                }
            }
        }

        best.map(|(_, aumid)| aumid).context(
            "未找到已安装的 Codex / ChatGPT 应用；请确认已从 Microsoft Store 安装，或通过 CODEX_PLUS_AUMID 手动指定",
        )
    }

    fn package_score(package_name: &str, display_name: &str) -> Option<u32> {
        let package_name = package_name.to_ascii_lowercase();
        let display_name = display_name.to_ascii_lowercase();

        if package_name.starts_with("openai.codex") {
            Some(100)
        } else if package_name.contains("openai.codex") {
            Some(90)
        } else if display_name.contains("codex") {
            Some(80)
        } else if display_name == "chatgpt" {
            Some(70)
        } else if display_name.contains("chatgpt") || package_name.contains("chatgpt") {
            Some(60)
        } else {
            None
        }
    }

    /// `OpenAI.Codex_26.1111.2228.0_x64__2p2nqsd0c76g0`
    /// -> `OpenAI.Codex_2p2nqsd0c76g0`
    fn package_family_name(package_full_name: &str) -> Option<String> {
        let name = package_full_name.split('_').next()?;
        let publisher_id = package_full_name.rsplit('_').next()?;
        if name.is_empty() || publisher_id.is_empty() {
            return None;
        }
        Some(format!("{name}_{publisher_id}"))
    }

    #[cfg(test)]
    mod tests {
        use super::{package_family_name, package_score};

        #[test]
        fn derives_family_name_from_full_name() {
            assert_eq!(
                package_family_name("OpenAI.Codex_26.1111.2228.0_x64__2p2nqsd0c76g0").as_deref(),
                Some("OpenAI.Codex_2p2nqsd0c76g0")
            );
        }

        #[test]
        fn prefers_the_codex_package() {
            assert_eq!(package_score("OpenAI.Codex_1.0_x64__abc", ""), Some(100));
            assert_eq!(
                package_score("Some.Other_1.0_x64__abc", "ChatGPT"),
                Some(70)
            );
            assert_eq!(package_score("Some.Other_1.0_x64__abc", "Notepad"), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate;

    #[test]
    fn accepts_a_well_formed_aumid() {
        assert_eq!(
            validate(" OpenAI.Codex_2p2nqsd0c76g0!App ").unwrap(),
            "OpenAI.Codex_2p2nqsd0c76g0!App"
        );
    }

    #[test]
    fn rejects_aumids_without_application_id() {
        assert!(validate("OpenAI.Codex_2p2nqsd0c76g0").is_err());
    }
}
