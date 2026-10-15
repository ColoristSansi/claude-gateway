//! 声明式供应商表：差异只允许出现在这里（路径模板 / 认证方式 / quirks 枚举）。
//! 转换代码里不允许出现供应商名判断。

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wire {
    Anthropic,
    OpenAi,
}

impl Wire {
    pub fn as_str(&self) -> &'static str {
        match self {
            Wire::Anthropic => "anthropic",
            Wire::OpenAi => "openai",
        }
    }
    pub fn parse(s: &str) -> Option<Wire> {
        match s {
            "anthropic" => Some(Wire::Anthropic),
            "openai" => Some(Wire::OpenAi),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Auth {
    Bearer,
    XApiKey,
}

impl Auth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Auth::Bearer => "bearer",
            Auth::XApiKey => "x-api-key",
        }
    }
    pub fn parse(s: &str) -> Auth {
        match s {
            "x-api-key" => Auth::XApiKey,
            _ => Auth::Bearer,
        }
    }
}

/// quirks 只能是枚举值；新增一个必须同时新增一条回归 fixture（票 09 的约束）。
#[derive(Debug, Clone)]
pub enum Quirk {
    /// 上游要求客户端自称（形如 `my-coding-agent/1.0`），不接受通用 SDK UA。
    ClientUserAgent(&'static str),
    /// 同一段对话必须带稳定会话 ID 的头名（值为网关按首条 user 文本派生）。
    SessionIdHeader(&'static str),
    /// 该上游的 Claude 模型只有 Anthropic 端点，发到 Chat 端点会 400。
    ClaudeModelsOnAnthropicOnly,
}

impl Quirk {
    /// 枚举值的全集：双向守卫拿它跟 fixture 标签对齐（票 19 §6）。
    pub fn all() -> Vec<Quirk> {
        vec![
            Quirk::ClientUserAgent(""),
            Quirk::SessionIdHeader(""),
            Quirk::ClaudeModelsOnAnthropicOnly,
        ]
    }

    /// 标签名（不含载荷）：fixture 的 `meta.json` 里写这个。
    pub fn name(&self) -> &'static str {
        match self {
            Quirk::ClientUserAgent(_) => "ClientUserAgent",
            Quirk::SessionIdHeader(_) => "SessionIdHeader",
            Quirk::ClaudeModelsOnAnthropicOnly => "ClaudeModelsOnAnthropicOnly",
        }
    }

    /// 票 19 §6.2：每个 quirk 必须声明它作用的出站 wire 或请求头。
    /// 用 `match` 而不是一张表——新增枚举值时编译器会逼着在这里补一行。
    pub fn acts_on(&self) -> &'static str {
        match self {
            Quirk::ClientUserAgent(_) => "出站请求头 user-agent（两种 wire 都注入）",
            Quirk::SessionIdHeader(_) => "出站请求头（头名由变体载荷给出，两种 wire 都注入）",
            Quirk::ClaudeModelsOnAnthropicOnly => "出站 wire：只许 anthropic",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Upstream {
    pub id: String,
    pub base_url: String,
    pub auth: Auth,
    pub api_key: String,
    pub path_anthropic: String,
    pub path_openai: String,
    pub quirks: Vec<Quirk>,
}

impl Upstream {
    pub fn path(&self, wire: Wire) -> &str {
        match wire {
            Wire::Anthropic => &self.path_anthropic,
            Wire::OpenAi => &self.path_openai,
        }
    }

    pub fn url(&self, wire: Wire) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), self.path(wire))
    }

    pub fn has_quirk(&self, q: &Quirk) -> bool {
        self.quirks
            .iter()
            .any(|x| std::mem::discriminant(x) == std::mem::discriminant(q))
    }

    /// 会话 ID：同一段对话稳定即可（上游用于路由与 prompt 缓存优化）。FNV-1a 够用。
    pub fn session_id(&self, seed: &str) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in seed.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
        format!("gw-{h:016x}")
    }
}

// ---------- env ----------

#[derive(Default)]
pub struct Env {
    pub vars: HashMap<String, String>,
    pub port: Option<u16>,
}

impl Env {
    /// 极简 KEY=VALUE 解析：只要 5 个键，不值得为它引一个 dotenv 依赖。
    pub fn load(path: &str) -> Env {
        let mut e = Env::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("[warn] 读不到 {path}，真实上游将不可用（只有 mock 能跑）");
            return e;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                e.vars.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
        e.port = e.get("GATEWAY_PORT").and_then(|v| v.parse().ok());
        e
    }

    pub fn get(&self, k: &str) -> Option<String> {
        self.vars.get(k).filter(|v| !v.is_empty()).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
    }

    /// 每份 `<scenario>/<wire>.meta.json`。
    fn fixture_metas(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(scenarios) = std::fs::read_dir(dir) else {
            return out;
        };
        for scenario in scenarios.flatten().filter(|e| e.path().is_dir()) {
            let Ok(files) = std::fs::read_dir(scenario.path()) else {
                continue;
            };
            for f in files.flatten() {
                let p = f.path();
                let name = p
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                if name.ends_with(".meta.json") {
                    out.push(p);
                }
            }
        }
        out
    }

    /// 每份 meta.json 的 `quirks` 标签 = 它覆盖的 quirk 名。
    fn tagged_quirks(dir: &Path) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for p in fixture_metas(dir) {
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            for n in v
                .get("quirks")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
            {
                out.push((n.to_string(), p.display().to_string()));
            }
        }
        out
    }

    /// 豁免表：只有真有理由的 quirk 才准留在这里，理由写在值里。
    fn exemptions(dir: &Path) -> HashMap<String, String> {
        let Ok(text) = std::fs::read_to_string(dir.join("quirk-exempt.json")) else {
            return HashMap::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return HashMap::new();
        };
        v.get("exempt")
            .and_then(serde_json::Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn every_quirk_declares_what_it_acts_on() {
        for q in Quirk::all() {
            assert!(!q.acts_on().is_empty(), "quirk {} 没声明作用对象", q.name());
        }
    }

    /// 双向守卫（票 19 §6.1）：枚举值 ⇄ fixture 标签，任一侧多出来都红。
    /// 反向验证（票 30 第 12 条）：抽掉一份带标签的 fixture，这条就变红。
    #[test]
    fn every_quirk_is_covered_by_a_fixture_or_explicitly_exempt() {
        let dir = fixtures_dir();
        let tags = tagged_quirks(&dir);
        let exempt = exemptions(&dir);
        assert!(
            !tags.is_empty(),
            "fixtures 里没有任何 quirk 标签，守卫等于没网"
        );
        for q in Quirk::all() {
            let n = q.name();
            assert!(
                tags.iter().any(|(t, _)| t == n) || exempt.contains_key(n),
                "quirk {n} 既没有 fixture 的 quirks 标签，也没写进 fixtures/quirk-exempt.json"
            );
        }
        for (name, why) in &exempt {
            assert!(
                Quirk::all().iter().any(|q| q.name() == name),
                "豁免表里的 {name} 不是现存枚举值"
            );
            assert!(!why.trim().is_empty(), "豁免 {name} 没写理由");
        }
    }

    /// 标签写错字也算守卫失败：否则枚举值能靠一个拼错的标签「假装」被覆盖。
    #[test]
    fn fixture_quirk_tags_are_real_enum_values() {
        for (tag, path) in tagged_quirks(&fixtures_dir()) {
            assert!(
                Quirk::all().iter().any(|q| q.name() == tag),
                "{path} 标了不存在的 quirk {tag}"
            );
        }
    }

    /// 标签指着的 fixture 必须真有字节：抽掉一份 `.sse`，守卫就得红（票 30 第 12 条的反向验证）。
    #[test]
    fn every_tagged_fixture_has_bytes() {
        let dir = fixtures_dir();
        for meta in fixture_metas(&dir) {
            let name = meta
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let sse = meta.with_file_name(name.replace(".meta.json", ".sse"));
            assert!(sse.exists(), "{} 没有对应的字节文件", sse.display());
            let Ok(text) = std::fs::read_to_string(&meta) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                panic!("{} 不是合法 JSON", meta.display());
            };
            for key in ["scenario", "outbound_wire", "model", "request"] {
                assert!(
                    !v.get(key).unwrap_or(&serde_json::Value::Null).is_null(),
                    "{} 缺字段 {key}",
                    meta.display()
                );
            }
        }
    }
}
