//! 写入引擎：接管键清单 → 冲突检测 → 备份 → 原子替换 → 记库 + 日志。
//!
//! 顺序与判据全部来自 ADR-0006 / spec §8.4，逐条对应：
//!
//! | 规则 | 落点 |
//! | --- | --- |
//! | 冲突只看接管键，冲突 = 放弃 + 提示 | [`Writer::write_one`] 的 [`Snapshot`] 比对 |
//! | 记录缺失 = 首次接管（先备份、再写、提示） | `first_write` 分支 |
//! | 幂等：语义相同完全不写（mtime 不变） | `satisfies` 全真即返回 [`FileOutcome::Unchanged`] |
//! | 还原默认摘键；整份回滚仅当未被外部改过 | [`Writer::restore_one`] / [`Writer::rollback`] |
//! | 多文件按依赖序、中途失败列出已改文件 | [`Writer::write`] / [`Writer::restore`] |
//!
//! 并发：单实例互斥量在应用启动时（外壳）保证只有一个写入方，进程内再加**每文件一把锁**
//! （[`lock_for`]，同一路径串行）；不做跨进程锁文件（ADR-0006 第 8 条）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value as Json;
use tokio::sync::Mutex as AsyncMutex;

use crate::store::Store;

use super::backup::{self, Backup};
use super::doc::{json_eq, Doc};
use super::log;
use super::plan::{
    ConflictMode, FileOutcome, FileSpec, RollbackOutcome, WritePlan, WriteReport, WriteRoots,
};
use super::state::{self, Snapshot};

/// 配置写入引擎。一个实例 = 一个写入根 + 一个存储句柄；可克隆使用。
#[derive(Clone)]
pub struct Writer {
    store: Store,
    roots: WriteRoots,
}

impl Writer {
    pub fn new(store: Store, roots: WriteRoots) -> Writer {
        Writer { store, roots }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn roots(&self) -> &WriteRoots {
        &self.roots
    }

    /// 目标文件的绝对路径（配置根 + 计划里的相对路径）。
    pub fn abs_path(&self, spec: &FileSpec) -> PathBuf {
        self.roots.config_root.join(&spec.rel_path)
    }

    /// 按依赖序写一份计划里的所有文件。
    pub async fn write(&self, plan: &WritePlan) -> Result<WriteReport> {
        let mut specs: Vec<&FileSpec> = plan.files.iter().collect();
        specs.sort_by_key(|s| s.order);
        let mut done: Vec<(PathBuf, FileOutcome)> = Vec::new();
        for spec in specs {
            match self.write_one(spec, plan.mode).await {
                Ok(o) => done.push((spec.rel_path.clone(), o)),
                Err(e) => {
                    return Err(e.context(format!(
                        "多文件写入中途失败；已改动：{}",
                        changed_list(&done)
                    )))
                }
            }
        }
        Ok(WriteReport { files: done })
    }

    /// 写一个「客户端 × 文件」（多文件计划的单文件版）。
    pub async fn write_one(&self, spec: &FileSpec, mode: ConflictMode) -> Result<FileOutcome> {
        let abs = self.abs_path(spec);
        let rel = rel_key(&spec.rel_path);
        let _guard = lock_for(&abs).lock_owned().await;

        let existed = abs.exists();
        let bytes = read_optional(&abs)?;
        // 解析失败即停：什么都不写，报错带行列位置。
        let mut doc = Doc::parse(spec.format, bytes.as_deref())?;
        let rec = state::load(&self.store, &spec.client, &rel).await?;

        // 冲突检测：只比接管键现值与记录值（非接管键、别的程序写的段不算冲突）。
        let mut notice = None;
        if let Some(rec) = &rec {
            if rec.snapshot.labels == spec.labels() {
                if let Some((i, current)) = mismatch(&rec.snapshot, spec, &doc) {
                    if mode == ConflictMode::Abort {
                        self.log(&format!(
                            "conflict client={} file={rel} key={}",
                            spec.client,
                            spec.write_ops[i].path().join(".")
                        ));
                        return Ok(FileOutcome::Conflict {
                            path: spec.write_ops[i].path().to_vec(),
                            recorded: rec.snapshot.values.get(i).cloned().unwrap_or(Json::Null),
                            current,
                        });
                    }
                    notice =
                        Some("接管键被外部改过，按「认领并覆盖」写入（只动接管键）".to_string());
                }
            } else {
                notice = Some("接管键清单与上次记录不符，按首次接管处理（先备份）".to_string());
            }
        }

        // 幂等：接管键值已经就是目标 → 完全不写（mtime 都不变）。
        if spec.write_ops.iter().all(|op| doc.satisfies(op)) {
            return Ok(FileOutcome::Unchanged);
        }

        let first_write = rec.is_none();
        let keep = rec.as_ref().and_then(|r| r.backup_path.clone());
        let bk = self.backup_for(spec, bytes.as_deref(), keep.as_deref())?;

        for op in &spec.write_ops {
            doc.apply(op)
                .with_context(|| format!("改写接管键失败（{}）", op.label()))?;
        }
        let out = doc.to_bytes()?;
        super::atomic::replace(&abs, &out).map_err(|e| self.write_failed(&abs, &bk, e))?;

        let snap = snapshot_of(&doc, spec);
        state::save(
            &self.store,
            &spec.client,
            &rel,
            &snap,
            "write",
            Some(&bk.path),
            existed,
        )
        .await?;

        if first_write {
            notice = Some(if bk.marker_only {
                format!(
                    "首次接管：原先没有这个文件，已记 marker {}",
                    bk.path.display()
                )
            } else {
                format!("首次接管：原文件已备份到 {}", bk.path.display())
            });
        }
        self.log(&format!(
            "write client={} file={rel} backup={} first={first_write} prev={}",
            spec.client,
            bk.path.display(),
            rec.as_ref().map(|r| r.last_op.as_str()).unwrap_or("-")
        ));
        Ok(FileOutcome::Written {
            backup: bk.path,
            marker_only: bk.marker_only,
            first_write,
            notice,
        })
    }

    /// 摘键还原（默认还原动作，spec §8.4 第 8 条）。
    pub async fn restore(&self, plan: &WritePlan) -> Result<WriteReport> {
        let mut specs: Vec<&FileSpec> = plan.files.iter().collect();
        specs.sort_by_key(|s| s.order);
        let mut done: Vec<(PathBuf, FileOutcome)> = Vec::new();
        for spec in specs {
            match self.restore_one(spec).await {
                Ok(o) => done.push((spec.rel_path.clone(), o)),
                Err(e) => {
                    return Err(e.context(format!(
                        "多文件还原中途失败；已改动：{}",
                        changed_list(&done)
                    )))
                }
            }
        }
        Ok(WriteReport { files: done })
    }

    pub async fn restore_one(&self, spec: &FileSpec) -> Result<FileOutcome> {
        // 空清单 = 调用方没给还原动作：宁可响亮报错，也不要「点了还原什么都没发生」。
        if spec.restore_ops.is_empty() {
            bail!(
                "这份接管键清单没有还原动作（{} 的 restore_ops 为空）",
                spec.rel_path.display()
            );
        }
        let abs = self.abs_path(spec);
        let rel = rel_key(&spec.rel_path);
        let _guard = lock_for(&abs).lock_owned().await;

        let Some(bytes) = read_optional(&abs)? else {
            return Ok(FileOutcome::Unchanged);
        };
        let mut doc = Doc::parse(spec.format, Some(&bytes))?;
        if spec.restore_ops.iter().all(|op| doc.satisfies(op)) {
            return Ok(FileOutcome::Unchanged);
        }
        let rec = state::load(&self.store, &spec.client, &rel).await?;
        let keep = rec.as_ref().and_then(|r| r.backup_path.clone());
        let bk = self.backup_for(spec, Some(&bytes), keep.as_deref())?;

        for op in &spec.restore_ops {
            doc.apply(op)
                .with_context(|| format!("摘键还原失败（{}）", op.label()))?;
        }
        // 网关当初新建、且现在只剩网关内容的文件 → 整个删（spec §8.4 第 8 条）。
        let gateway_created = rec
            .as_ref()
            .map(|r| !r.existed_at_first_write)
            .unwrap_or(false);
        let removed = gateway_created && doc.is_empty();
        if removed {
            fs::remove_file(&abs).map_err(|e| self.write_failed(&abs, &bk, e))?;
        } else {
            let out = doc.to_bytes()?;
            super::atomic::replace(&abs, &out).map_err(|e| self.write_failed(&abs, &bk, e))?;
        }

        let snap = snapshot_of(&doc, spec);
        state::save(
            &self.store,
            &spec.client,
            &rel,
            &snap,
            "restore",
            Some(&bk.path),
            true,
        )
        .await?;
        self.log(&format!(
            "restore client={} file={rel} backup={} removed={removed}",
            spec.client,
            bk.path.display()
        ));
        Ok(if removed {
            FileOutcome::Removed {
                backup: bk.path,
                marker_only: bk.marker_only,
            }
        } else {
            FileOutcome::Written {
                backup: bk.path,
                marker_only: bk.marker_only,
                first_write: false,
                notice: Some("已摘掉网关接管键".to_string()),
            }
        })
    }

    /// 整份回滚（显式第二选项）：从首写快照按字节恢复。
    /// **仅当文件自上次网关写入后未被外部改过**（同一套接管键判据），否则拒绝。
    pub async fn rollback(&self, spec: &FileSpec) -> Result<RollbackOutcome> {
        let abs = self.abs_path(spec);
        let rel = rel_key(&spec.rel_path);
        let _guard = lock_for(&abs).lock_owned().await;

        let Some(rec) = state::load(&self.store, &spec.client, &rel).await? else {
            return Ok(RollbackOutcome::Refused {
                reason: "没有网关写入记录，无法整份回滚".to_string(),
            });
        };
        if rec.snapshot.labels != spec.labels() {
            return Ok(RollbackOutcome::Refused {
                reason: "接管键清单与记录不符（清单变过），拒绝整份回滚".to_string(),
            });
        }
        let bytes = read_optional(&abs)?;
        let doc = Doc::parse(spec.format, bytes.as_deref())?;
        if let Some((i, current)) = mismatch(&rec.snapshot, spec, &doc) {
            return Ok(RollbackOutcome::Refused {
                reason: format!(
                    "文件自上次网关写入后被外部改过（{} 现值 {} ≠ 记录 {}），拒绝整份回滚",
                    spec.write_ops[i].path().join("."),
                    current,
                    rec.snapshot.values.get(i).unwrap_or(&Json::Null)
                ),
            });
        }
        let Some(snapshot_path) = rec.backup_path.clone() else {
            return Ok(RollbackOutcome::Refused {
                reason: "没有首写快照路径，无法整份回滚".to_string(),
            });
        };
        if !snapshot_path.is_file() {
            return Ok(RollbackOutcome::Refused {
                reason: format!("首写快照不在了：{}", snapshot_path.display()),
            });
        }
        if bytes.is_none() {
            return Ok(RollbackOutcome::Unchanged);
        }

        // 网关当初新建的文件：回滚 = 整个删掉。
        if !rec.existed_at_first_write {
            let bk = self.backup_for(spec, bytes.as_deref(), Some(&snapshot_path))?;
            fs::remove_file(&abs).map_err(|e| self.write_failed(&abs, &bk, e))?;
            let snap = snapshot_of(&Doc::empty(spec.format), spec);
            state::save(
                &self.store,
                &spec.client,
                &rel,
                &snap,
                "restore",
                Some(&snapshot_path),
                false,
            )
            .await?;
            self.log(&format!(
                "rollback client={} file={rel} deleted=true snapshot={}",
                spec.client,
                snapshot_path.display()
            ));
            return Ok(RollbackOutcome::Rolled {
                backup: snapshot_path,
                deleted: true,
            });
        }

        let snapshot_bytes = fs::read(&snapshot_path)
            .with_context(|| format!("读首写快照失败：{}", snapshot_path.display()))?;
        // 回滚也是一次写入：照规矩先留一份滚动备份（首写快照不受影响）。
        self.backup_for(spec, bytes.as_deref(), Some(&snapshot_path))?;
        super::atomic::replace(&abs, &snapshot_bytes).map_err(|e| {
            anyhow!(
                "整份回滚写回失败：目标文件 {}；快照 {}；{}",
                abs.display(),
                snapshot_path.display(),
                e
            )
        })?;
        let restored = Doc::parse(spec.format, Some(&snapshot_bytes))?;
        let snap = snapshot_of(&restored, spec);
        state::save(
            &self.store,
            &spec.client,
            &rel,
            &snap,
            "restore",
            Some(&snapshot_path),
            true,
        )
        .await?;
        self.log(&format!(
            "rollback client={} file={rel} deleted=false snapshot={}",
            spec.client,
            snapshot_path.display()
        ));
        Ok(RollbackOutcome::Rolled {
            backup: snapshot_path,
            deleted: false,
        })
    }

    /// 写前留备份（首写 = 首写快照 / marker；之后 = 滚动备份）。
    fn backup_for(
        &self,
        spec: &FileSpec,
        bytes: Option<&[u8]>,
        keep: Option<&Path>,
    ) -> Result<Backup> {
        let name = file_name_of(&self.abs_path(spec))?;
        backup::snapshot(&self.roots.backup_root, &spec.client, &name, bytes, keep).with_context(
            || {
                format!(
                    "写备份失败：{}（客户端 {}）",
                    self.roots.backup_root.join(&spec.client).display(),
                    spec.client
                )
            },
        )
    }

    /// 占用 / 权限类失败的统一报错：**目标文件路径 + 备份路径 + 占用提示**（spec §8.4 第 2 条）。
    fn write_failed(&self, target: &Path, bk: &Backup, e: std::io::Error) -> anyhow::Error {
        anyhow!(
            "写客户端配置失败：目标文件 {}；备份 {}；{e}。可能有客户端正占用该文件，请完全退出该客户端后重试。",
            target.display(),
            bk.path.display()
        )
    }

    fn log(&self, line: &str) {
        // 日志失败不许把写入带崩。
        let _ = log::append(&self.roots.log_root, line);
    }
}

/// 每文件一把锁（同一路径的写串行化）。跨进程不做锁文件（ADR-0006 第 8 条）。
fn lock_for(abs: &Path) -> Arc<AsyncMutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> = OnceLock::new();
    let map = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(abs.to_path_buf()).or_default().clone()
}

/// 记录/比对用的相对路径（统一正斜杠，跨根稳定）。
fn rel_key(rel: &Path) -> String {
    rel.to_string_lossy().replace('\\', "/")
}

fn file_name_of(abs: &Path) -> Result<String> {
    abs.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .ok_or_else(|| anyhow!("目标文件没有文件名：{}", abs.display()))
}

fn read_optional(abs: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(abs) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("读目标文件失败：{}", abs.display())),
    }
}

/// 接管键现值与记录值的第一个不一致（= 漂移 / 冲突）。
fn mismatch(rec: &Snapshot, spec: &FileSpec, doc: &Doc) -> Option<(usize, Json)> {
    for (i, op) in spec.write_ops.iter().enumerate() {
        let current = doc.slot(op);
        let recorded = rec.values.get(i).unwrap_or(&Json::Null);
        if !json_eq(recorded, &current) {
            return Some((i, current));
        }
    }
    None
}

/// 本次动作之后，接管键的槽值快照（记进 `client_config_state`）。
fn snapshot_of(doc: &Doc, spec: &FileSpec) -> Snapshot {
    Snapshot {
        labels: spec.labels(),
        values: spec.write_ops.iter().map(|op| doc.slot(op)).collect(),
    }
}

/// 已改动的文件清单（多文件中途失败时报给用户）。
fn changed_list(done: &[(PathBuf, FileOutcome)]) -> String {
    let changed: Vec<String> = done
        .iter()
        .filter(|(_, o)| !matches!(o, FileOutcome::Unchanged | FileOutcome::Conflict { .. }))
        .map(|(p, _)| p.display().to_string())
        .collect();
    if changed.is_empty() {
        "（无）".to_string()
    } else {
        changed.join("、")
    }
}
