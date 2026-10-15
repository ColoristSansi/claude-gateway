//! 文档层：三种格式的解析 / 取值 / **保序改写** / 序列化（spec §8 第 1 条、§8.4 第 5 条）。
//!
//! 三条硬约束在这里落地：
//!
//! - **解析失败即停**：报错带行列位置，绝不退回空文档重写（票 28 第 1 条第 3 点）。
//! - **只动接管键**：清单外的键、别的工具写的段、用户注释一律原样保留（TOML 用
//!   `toml_edit`，连用户注释都不洗——ADR-0006 只要求「接受重序列化」，保下来更好）。
//! - **保序**：JSON 用 `preserve_order` 的 `IndexMap`、删除走位移语义；TOML 保文档顺序。
//!
//! 保真（缩进 / 行尾 / BOM）取自目标文件本身：ISO-8859 的 BOM 保留、CRLF 文件写回 CRLF。

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value as Json;
use toml_edit::{DocumentMut, InlineTable, Item, Table, Value as TomlValue};

use super::plan::{Format, OwnedOp};

/// 原文件的排版保真信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fidelity {
    /// 原文件带 UTF-8 BOM。
    pub bom: bool,
    /// 原文件行尾是 CRLF（本机实测客户端配置都是 CRLF）。
    pub crlf: bool,
    /// JSON 的缩进单元（从原文件第一条缩进行认出来；认不出就用两空格）。
    pub indent: String,
    /// 原文件末尾有换行。
    pub trailing_newline: bool,
}

impl Default for Fidelity {
    fn default() -> Fidelity {
        Fidelity {
            bom: false,
            crlf: false,
            indent: "  ".to_string(),
            trailing_newline: true,
        }
    }
}

impl Fidelity {
    /// 从**原始字节**认排版（解析之前做，因为解析器不保留这些）。
    pub fn detect(bytes: &[u8]) -> Fidelity {
        let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let text = String::from_utf8_lossy(bytes);
        let crlf = text.contains("\r\n");
        let trailing_newline = text.ends_with('\n');
        Fidelity {
            bom,
            crlf,
            indent: detect_indent(&text).unwrap_or_else(|| "  ".to_string()),
            trailing_newline,
        }
    }
}

/// 缩进单元：找第一条「以上一行的 `{` / `[` 收尾、自己又是缩进行」的行，取它的前导空白。
fn detect_indent(text: &str) -> Option<String> {
    let mut prev_open = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        let lead: String = line[..line.len() - trimmed.len()].to_string();
        if prev_open && !lead.is_empty() {
            return Some(lead);
        }
        prev_open = trimmed.ends_with('{') || trimmed.ends_with('[');
    }
    None
}

/// 解析后的文档 + 它的排版信息。
pub struct Doc {
    inner: Inner,
    pub fidelity: Fidelity,
}

enum Inner {
    Json(Json),
    Toml(Box<DocumentMut>),
}

impl Doc {
    /// 解析目标文件。`bytes = None` 表示文件不存在（空文档：JSON `{}`、TOML 空表）。
    pub fn parse(format: Format, bytes: Option<&[u8]>) -> Result<Doc> {
        let Some(bytes) = bytes else {
            return Ok(Doc::empty(format));
        };
        let fidelity = Fidelity::detect(bytes);
        let body = strip_bom(bytes);
        let text = std::str::from_utf8(body)
            .map_err(|e| anyhow!("配置文件不是合法 UTF-8（偏移 {}）：{e}", e.valid_up_to()))?;
        match format {
            Format::Json | Format::Jsonc => {
                let cleaned = match format {
                    Format::Jsonc => strip_jsonc(text)?,
                    _ => text.to_string(),
                };
                let value: Json = serde_json::from_str(&cleaned).map_err(|e| {
                    anyhow!(
                        "解析失败即停：JSON 第 {} 行第 {} 列：{}（文件未改动）",
                        e.line(),
                        e.column(),
                        e
                    )
                })?;
                Ok(Doc {
                    inner: Inner::Json(value),
                    fidelity,
                })
            }
            Format::Toml => {
                let doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
                    let (line, col) = e.span().map(|s| line_col(text, s.start)).unwrap_or((0, 0));
                    anyhow!("解析失败即停：TOML 第 {line} 行第 {col} 列：{e}（文件未改动）")
                })?;
                Ok(Doc {
                    inner: Inner::Toml(Box::new(doc)),
                    fidelity,
                })
            }
        }
    }

    /// 空文档（文件不存在 / 网关新建）。
    pub fn empty(format: Format) -> Doc {
        match format {
            Format::Json | Format::Jsonc => Doc {
                inner: Inner::Json(Json::Object(Default::default())),
                fidelity: Fidelity::default(),
            },
            Format::Toml => Doc {
                inner: Inner::Toml(Box::new(DocumentMut::new())),
                fidelity: Fidelity::default(),
            },
        }
    }

    /// 取接管键现值（归一成 JSON 值；缺席 = `None`）。
    pub fn get(&self, path: &[String]) -> Option<Json> {
        match &self.inner {
            Inner::Json(v) => json_at(v, path).cloned(),
            Inner::Toml(d) => toml_path_json(d.as_table(), path),
        }
    }

    /// 这条动作现在满足了吗（满足就**完全不写**，spec §8.4 第 3 条）。
    pub fn satisfies(&self, op: &OwnedOp) -> bool {
        let path = op.path();
        match op {
            OwnedOp::Set { value, .. } => self
                .get(path)
                .map(|cur| json_eq(&cur, value))
                .unwrap_or(false),
            OwnedOp::Remove { .. } | OwnedOp::RemoveIfEmpty { .. } => self.get(path).is_none(),
            OwnedOp::SetElement { field, value, .. } => match value.get(field) {
                Some(want) => self
                    .element(path, field, want)
                    .map(|e| json_eq(&e, value))
                    .unwrap_or(false),
                None => false,
            },
            OwnedOp::RemoveElement { field, matches, .. } => {
                self.element(path, field, matches).is_none()
            }
            OwnedOp::ReplaceIfEquals { equals, .. } => self.get(path).as_ref() != Some(equals),
        }
    }

    /// 这条接管键上**还留着网关写下去的值**吗（四态的「没有网关键」判据用它）。
    ///
    /// 与 [`Doc::satisfies`] 不能合并：`satisfies` 是幂等判据（「现在正是网关要写的样子」），
    /// 这里判的是「网关的值还在不在」。用户手改过的键两者都是 `false`，但手改**不代表**网关没写过——
    /// 所以四态判定用的是「一个文件的接管键里还有没有网关的值」，不是「键在不在」。
    ///
    /// 只有「写下去一个值」的动作（`Set` / `SetElement`）才有「网关的值」可言；摘键类动作
    /// 本来就不留值。`appliedId` 是普通 `Set`，还原后变成客户端自己的档案 id → 判 `false`：
    /// 键还在、网关的值不在了，这正是「已回官方」的判据。
    pub fn carries_gateway_value(&self, op: &OwnedOp) -> bool {
        op.expected().is_some() && self.satisfies(op)
    }

    /// 冲突比对用的「这条键现在的槽值」（缺席 = `null`）——记录里存的是同一个口径。
    pub fn slot(&self, op: &OwnedOp) -> Json {
        match op {
            OwnedOp::SetElement { field, value, .. } => value
                .get(field)
                .and_then(|want| self.element(op.path(), field, want))
                .unwrap_or(Json::Null),
            OwnedOp::RemoveElement { field, matches, .. } => self
                .element(op.path(), field, matches)
                .unwrap_or(Json::Null),
            _ => self.get(op.path()).unwrap_or(Json::Null),
        }
    }

    /// 数组 `path` 里第一个 `elem[field] == want` 的元素。
    fn element(&self, path: &[String], field: &str, want: &Json) -> Option<Json> {
        let arr = self.get(path)?;
        let arr = arr.as_array()?;
        arr.iter()
            .find(|e| e.get(field).map(|v| json_eq(v, want)).unwrap_or(false))
            .cloned()
    }

    /// 应用一条动作（保序改写；形状不对就报错，不猜）。
    pub fn apply(&mut self, op: &OwnedOp) -> Result<()> {
        match op {
            OwnedOp::Set { path, value } => match &mut self.inner {
                Inner::Json(root) => {
                    let slot = json_ensure(root, path)?;
                    *slot = value.clone();
                    Ok(())
                }
                Inner::Toml(doc) => {
                    let slot = toml_ensure(doc.as_table_mut(), path)?;
                    *slot = json_to_item(value)?;
                    Ok(())
                }
            },
            OwnedOp::Remove { path } => {
                match &mut self.inner {
                    Inner::Json(root) => {
                        json_remove(root, path)?;
                    }
                    Inner::Toml(doc) => {
                        toml_remove(doc.as_table_mut(), path)?;
                    }
                }
                Ok(())
            }
            OwnedOp::RemoveIfEmpty { path } => self.remove_if_empty(path),
            OwnedOp::SetElement { path, field, value } => {
                let want = value
                    .get(field)
                    .cloned()
                    .ok_or_else(|| anyhow!("SetElement 的值里没有字段 {field}：{value}"))?;
                match &mut self.inner {
                    Inner::Json(root) => {
                        let arr = json_ensure_array(root, path)?;
                        match arr
                            .iter()
                            .position(|e| e.get(field).map(|v| json_eq(v, &want)).unwrap_or(false))
                        {
                            Some(i) => arr[i] = value.clone(),
                            None => arr.push(value.clone()),
                        }
                    }
                    Inner::Toml(doc) => {
                        let arr = toml_ensure_array(doc.as_table_mut(), path)?;
                        let want_toml = json_to_toml_value(&want)?;
                        let mut replaced = false;
                        for e in arr.iter_mut() {
                            if inline_field_eq(e, field, &want_toml) {
                                *e = json_to_toml_value(value)?;
                                replaced = true;
                                break;
                            }
                        }
                        if !replaced {
                            arr.push(json_to_toml_value(value)?);
                        }
                    }
                }
                Ok(())
            }
            OwnedOp::RemoveElement {
                path,
                field,
                matches,
            } => {
                match &mut self.inner {
                    Inner::Json(root) => {
                        if let Some(arr) = json_array_mut(root, path)? {
                            arr.retain(|e| {
                                !e.get(field).map(|v| json_eq(v, matches)).unwrap_or(false)
                            });
                        }
                    }
                    Inner::Toml(doc) => {
                        if let Some(arr) = toml_array_mut(doc.as_table_mut(), path)? {
                            let want = json_to_toml_value(matches)?;
                            arr.retain(|e| !inline_field_eq(e, field, &want));
                        }
                    }
                }
                Ok(())
            }
            OwnedOp::ReplaceIfEquals {
                path,
                equals,
                value,
            } => {
                if self.get(path).as_ref() != Some(equals) {
                    return Ok(());
                }
                match value {
                    Some(v) => self.apply(&OwnedOp::Set {
                        path: path.clone(),
                        value: v.clone(),
                    }),
                    None => self.apply(&OwnedOp::Remove { path: path.clone() }),
                }
            }
        }
    }

    /// 摘键，并且把因此变空的空壳一层层摘掉（`env` 摘空即摘）。
    fn remove_if_empty(&mut self, path: &[String]) -> Result<()> {
        if path.is_empty() {
            return Ok(());
        }
        self.apply(&OwnedOp::Remove {
            path: path.to_vec(),
        })?;
        // 自底向上收空壳（上一层空了也摘）。
        for cut in (1..path.len()).rev() {
            let parent = &path[..cut];
            let empty_parent = match self.get(parent) {
                Some(Json::Object(m)) => m.is_empty(),
                _ => false,
            };
            if !empty_parent {
                break;
            }
            self.apply(&OwnedOp::Remove {
                path: parent.to_vec(),
            })?;
        }
        Ok(())
    }

    /// 删了网关接管键之后，文件里还剩东西吗（网关新建的文件整份删的判据）。
    pub fn is_empty(&self) -> bool {
        match &self.inner {
            Inner::Json(Json::Object(m)) => m.is_empty(),
            Inner::Json(Json::Null) => true,
            Inner::Json(_) => false,
            Inner::Toml(d) => d.as_table().is_empty(),
        }
    }

    /// 序列化回字节：沿用原文件的 BOM / 行尾 / 末尾换行。
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = self.to_text()?;
        if self.fidelity.trailing_newline && !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        let mut bytes = Vec::with_capacity(out.len() + 3);
        if self.fidelity.bom {
            bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        if self.fidelity.crlf {
            let raw = out.as_bytes();
            for (i, b) in raw.iter().enumerate() {
                if *b == b'\n' && (i == 0 || raw[i - 1] != b'\r') {
                    bytes.push(b'\r');
                }
                bytes.push(*b);
            }
        } else {
            bytes.extend_from_slice(out.as_bytes());
        }
        Ok(bytes)
    }

    fn to_text(&self) -> Result<String> {
        match &self.inner {
            Inner::Json(v) => {
                let mut buf = Vec::new();
                let fmt =
                    serde_json::ser::PrettyFormatter::with_indent(self.fidelity.indent.as_bytes());
                let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
                v.serialize(&mut ser).context("JSON 序列化失败")?;
                String::from_utf8(buf).context("JSON 序列化结果不是 UTF-8")
            }
            // toml_edit 保留原有键序、注释与别的段，只重写被改的键。
            Inner::Toml(d) => Ok(d.to_string()),
        }
    }
}

/// BOM 之后的正文字节。
fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes)
}

/// 字节偏移 → (行, 列)，行列都从 1 数（给人看的报错位置）。
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let upto = &text[..offset.min(text.len())];
    let line = upto.matches('\n').count() + 1;
    let col = upto
        .rsplit('\n')
        .next()
        .map(|s| s.chars().count())
        .unwrap_or(0)
        + 1;
    (line, col)
}

/// JSONC → JSON：把注释与尾逗号**原地换成空格**（换行保留），字节偏移不变，
/// 于是 `serde_json` 报的行列号仍然对应原文件。
fn strip_jsonc(src: &str) -> Result<String> {
    #[derive(PartialEq)]
    enum S {
        Normal,
        Str,
        Line,
        Block,
    }
    let bytes = src.as_bytes();
    let mut out = bytes.to_vec();
    let mut state = S::Normal;
    let mut pending_comma: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let next = bytes.get(i + 1).copied();
        match state {
            S::Normal => match (b, next) {
                (b'"', _) => {
                    state = S::Str;
                    pending_comma = None;
                }
                (b'/', Some(b'/')) => {
                    state = S::Line;
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    continue;
                }
                (b'/', Some(b'*')) => {
                    state = S::Block;
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    continue;
                }
                (b',', _) => pending_comma = Some(i),
                (b']', _) | (b'}', _) => {
                    if let Some(c) = pending_comma.take() {
                        out[c] = b' ';
                    }
                }
                (b' ', _) | (b'\t', _) | (b'\r', _) | (b'\n', _) => {}
                _ => pending_comma = None,
            },
            S::Str => match b {
                b'\\' => {
                    i += 2;
                    continue;
                }
                b'"' => state = S::Normal,
                _ => {}
            },
            S::Line => match b {
                b'\n' => state = S::Normal,
                _ => out[i] = b' ',
            },
            S::Block => match (b, next) {
                (b'*', Some(b'/')) => {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    state = S::Normal;
                    i += 2;
                    continue;
                }
                (b'\n', _) => {}
                _ => out[i] = b' ',
            },
        }
        i += 1;
    }
    if state == S::Block {
        bail!("解析失败即停：JSONC 注释块没有收尾（文件未改动）");
    }
    String::from_utf8(out).context("JSONC 清洗后不是 UTF-8")
}

// ── JSON 取值 / 改写 ─────────────────────────────────────────────────────────────

/// 数字按值比（TOML 的整数与 JSON 的 `1.0` 是同一个语义值）；其余递归比。
pub(crate) fn json_eq(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => x == y,
        },
        (Json::Array(x), Json::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(a, b)| json_eq(a, b))
        }
        (Json::Object(x), Json::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).map(|w| json_eq(v, w)).unwrap_or(false))
        }
        _ => a == b,
    }
}

fn json_at<'a>(root: &'a Json, path: &[String]) -> Option<&'a Json> {
    let mut cur = root;
    for seg in path {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

/// 走到 `path` 的槽位，缺的中间对象按需创建（保序：新键追加在末尾）。
fn json_ensure<'a>(root: &'a mut Json, path: &[String]) -> Result<&'a mut Json> {
    let Some((first, rest)) = path.split_first() else {
        return Ok(root);
    };
    if !root.is_object() {
        bail!("接管键路径上的 {first} 不是对象（值 = {root}），停写");
    }
    let obj = root.as_object_mut().expect("刚判过是对象");
    let mut cur = obj
        .entry(first.clone())
        .or_insert_with(|| Json::Object(Default::default()));
    for seg in rest {
        if !cur.is_object() {
            bail!("接管键路径上的 {seg} 不是对象（值 = {cur}），停写");
        }
        let obj = cur.as_object_mut().expect("刚判过是对象");
        cur = obj
            .entry(seg.clone())
            .or_insert_with(|| Json::Object(Default::default()));
    }
    Ok(cur)
}

fn json_remove(root: &mut Json, path: &[String]) -> Result<()> {
    let (last, parents) = match path.split_last() {
        Some(v) => v,
        None => return Ok(()),
    };
    let Some(parent) = json_at_mut(root, parents)? else {
        return Ok(());
    };
    if let Some(obj) = parent.as_object_mut() {
        // 位移删除：保住其余键的顺序（票 28 第 1 条第 2 点）。
        obj.shift_remove(last);
    } else {
        bail!("接管键路径上的 {last} 不是对象，停写");
    }
    Ok(())
}

fn json_at_mut<'a>(root: &'a mut Json, path: &[String]) -> Result<Option<&'a mut Json>> {
    let mut cur = root;
    for seg in path {
        let Some(obj) = cur.as_object_mut() else {
            bail!("接管键路径上的 {seg} 不是对象，停写");
        };
        match obj.get_mut(seg) {
            Some(v) => cur = v,
            None => return Ok(None),
        }
    }
    Ok(Some(cur))
}

fn json_ensure_array<'a>(root: &'a mut Json, path: &[String]) -> Result<&'a mut Vec<Json>> {
    let slot = json_ensure(root, path)?;
    // `json_ensure` 给**缺失**的键建的是空对象，`Set` 到过 `null` 的槽位是 null——两者都要归一成
    // 空数组。桌面写入器的 `_meta.json` 第一次被网关新建时，`entries` 走的正是这条路
    // （票 46：`SetElement` 必须能自己把数组建出来）。
    if slot.is_null() || slot.as_object().is_some_and(|o| o.is_empty()) {
        *slot = Json::Array(Vec::new());
    }
    slot.as_array_mut()
        .ok_or_else(|| anyhow!("接管键 {} 不是数组，停写", path.join(".")))
}

fn json_array_mut<'a>(root: &'a mut Json, path: &[String]) -> Result<Option<&'a mut Vec<Json>>> {
    let Some(slot) = json_at_mut(root, path)? else {
        return Ok(None);
    };
    match slot {
        Json::Array(a) => Ok(Some(a)),
        _ => bail!("接管键 {} 不是数组，停写", path.join(".")),
    }
}

// ── TOML 取值 / 改写 ────────────────────────────────────────────────────────────

fn toml_path_json(table: &Table, path: &[String]) -> Option<Json> {
    let (first, rest) = path.split_first()?;
    let item = table.get(first)?;
    if rest.is_empty() {
        return Some(item_to_json(item));
    }
    match item {
        Item::Table(t) => toml_path_json(t, rest),
        Item::Value(TomlValue::InlineTable(t)) => inline_path_json(t, rest),
        _ => None,
    }
}

fn inline_path_json(table: &InlineTable, path: &[String]) -> Option<Json> {
    let (first, rest) = path.split_first()?;
    let value = table.get(first)?;
    if rest.is_empty() {
        return Some(toml_value_to_json(value));
    }
    match value {
        TomlValue::InlineTable(t) => inline_path_json(t, rest),
        _ => None,
    }
}

/// TOML 的中间层：只认 `[表]`（内联表的内部不改，报错说清）。
fn toml_ensure<'a>(table: &'a mut Table, path: &[String]) -> Result<&'a mut Item> {
    let Some((first, rest)) = path.split_first() else {
        bail!("接管键路径不能为空");
    };
    if !table.contains_key(first) {
        table.insert(first, Item::Table(Table::new()));
    }
    let item = table
        .get_mut(first)
        .ok_or_else(|| anyhow!("TOML 键 {first} 建不出来"))?;
    if rest.is_empty() {
        return Ok(item);
    }
    let sub = item.as_table_mut().ok_or_else(|| {
        anyhow!("TOML 接管键路径上的 {first} 不是表（内联表内部本引擎不改），停写")
    })?;
    toml_ensure(sub, rest)
}

fn toml_remove(table: &mut Table, path: &[String]) -> Result<()> {
    let Some((last, parents)) = path.split_last() else {
        return Ok(());
    };
    let mut cur = table;
    for seg in parents {
        match cur.get_mut(seg) {
            Some(Item::Table(t)) => cur = t,
            Some(_) => bail!("TOML 接管键路径上的 {seg} 不是表，停写"),
            None => return Ok(()),
        }
    }
    cur.remove(last);
    Ok(())
}

fn toml_ensure_array<'a>(
    table: &'a mut Table,
    path: &[String],
) -> Result<&'a mut toml_edit::Array> {
    let item = toml_ensure(table, path)?;
    if item.is_none() {
        *item = Item::Value(TomlValue::Array(toml_edit::Array::new()));
    }
    item.as_value_mut()
        .and_then(|v| v.as_array_mut())
        .ok_or_else(|| anyhow!("TOML 接管键 {} 不是数组，停写", path.join(".")))
}

fn toml_array_mut<'a>(
    table: &'a mut Table,
    path: &[String],
) -> Result<Option<&'a mut toml_edit::Array>> {
    let Some((last, parents)) = path.split_last() else {
        return Ok(None);
    };
    let mut cur = table;
    for seg in parents {
        match cur.get_mut(seg) {
            Some(Item::Table(t)) => cur = t,
            Some(_) => bail!("TOML 接管键路径上的 {seg} 不是表，停写"),
            None => return Ok(None),
        }
    }
    match cur.get_mut(last) {
        None => Ok(None),
        Some(Item::Value(v)) => match v.as_array_mut() {
            Some(a) => Ok(Some(a)),
            None => bail!("TOML 接管键 {} 不是数组，停写", path.join(".")),
        },
        Some(_) => bail!("TOML 接管键 {} 不是数组，停写", path.join(".")),
    }
}

fn inline_field_eq(item: &TomlValue, field: &str, want: &TomlValue) -> bool {
    let got = match item {
        TomlValue::InlineTable(t) => t.get(field),
        _ => None,
    };
    match got {
        Some(g) => toml_eq(g, want),
        None => false,
    }
}

fn toml_eq(a: &TomlValue, b: &TomlValue) -> bool {
    toml_value_to_json(a) == toml_value_to_json(b)
        || json_eq(&toml_value_to_json(a), &toml_value_to_json(b))
}

fn item_to_json(item: &Item) -> Json {
    match item {
        Item::None => Json::Null,
        Item::Value(v) => toml_value_to_json(v),
        Item::Table(t) => table_to_json(t),
        Item::ArrayOfTables(a) => Json::Array(a.iter().map(table_to_json).collect()),
    }
}

fn table_to_json(t: &Table) -> Json {
    Json::Object(
        t.iter()
            .map(|(k, v)| (k.to_string(), item_to_json(v)))
            .collect(),
    )
}

fn toml_value_to_json(v: &TomlValue) -> Json {
    match v {
        TomlValue::String(s) => Json::String(s.value().clone()),
        TomlValue::Integer(i) => Json::Number((*i.value()).into()),
        TomlValue::Float(f) => serde_json::Number::from_f64(*f.value())
            .map(Json::Number)
            .unwrap_or(Json::Null),
        TomlValue::Boolean(b) => Json::Bool(*b.value()),
        TomlValue::Datetime(d) => Json::String(d.value().to_string()),
        TomlValue::Array(a) => Json::Array(a.iter().map(toml_value_to_json).collect()),
        TomlValue::InlineTable(t) => Json::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), toml_value_to_json(v)))
                .collect(),
        ),
    }
}

/// JSON 值 → TOML 项。对象写成**内联表**（键值形式，不改动所在表的层级）。
fn json_to_item(v: &Json) -> Result<Item> {
    Ok(Item::Value(json_to_toml_value(v)?))
}

fn json_to_toml_value(v: &Json) -> Result<TomlValue> {
    Ok(match v {
        Json::Null => bail!("TOML 没有 null 字面值：摘键请用 Remove，别 Set 成 null"),
        Json::Bool(b) => TomlValue::from(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                TomlValue::from(i)
            } else if let Some(f) = n.as_f64() {
                TomlValue::from(f)
            } else {
                bail!("TOML 装不下这个数：{n}")
            }
        }
        Json::String(s) => TomlValue::from(s.clone()),
        Json::Array(a) => {
            let mut arr = toml_edit::Array::new();
            for v in a {
                arr.push(json_to_toml_value(v)?);
            }
            TomlValue::Array(arr)
        }
        Json::Object(o) => {
            let mut t = InlineTable::new();
            for (k, val) in o {
                t.insert(k, json_to_toml_value(val)?);
            }
            TomlValue::InlineTable(t)
        }
    })
}
