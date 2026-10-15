//! `client_config_state`（票 42 建好的表）读写：写入引擎的唯一真相（ADR-0006 第 2 条）。
//!
//! 每个「客户端 × 文件」一行：上次由网关写入的**接管键值**、时间、首写快照路径、
//! 首写时文件是否存在。冲突检测、幂等、还原都读它，不另设数据目录 JSON 状态文件。
//!
//! `file_path` 存**相对配置根**的路径（写入根是参数化的，绝对路径会随根变化而失配）。

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use serde_json::Value as Json;
use std::path::{Path, PathBuf};

use crate::store::Store;

/// 记录下来的接管键快照：标签（判清单有没有变）+ 每个槽位的值（判漂移）。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Snapshot {
    pub labels: Vec<String>,
    pub values: Vec<Json>,
}

impl Snapshot {
    pub fn to_json(&self) -> String {
        serde_json::json!({ "v": 1, "labels": self.labels, "values": self.values }).to_string()
    }

    pub fn from_json(s: &str) -> Result<Snapshot> {
        let v: Json =
            serde_json::from_str(s).context("client_config_state.owned_keys_json 不是 JSON")?;
        let labels = v
            .get("labels")
            .and_then(|l| l.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let values = v
            .get("values")
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(Snapshot { labels, values })
    }
}

/// 一行 `client_config_state`。
#[derive(Clone, Debug)]
pub(crate) struct Record {
    pub snapshot: Snapshot,
    /// 上一次动作：`write` / `restore`（写进元数据日志，方便回看是「写坏了」还是「还原过」）。
    pub last_op: String,
    pub backup_path: Option<PathBuf>,
    pub existed_at_first_write: bool,
}

/// 读一行（没有记录 = 首次接管）。
pub(crate) async fn load(store: &Store, client: &str, rel: &str) -> Result<Option<Record>> {
    let (client, rel) = (client.to_string(), rel.to_string());
    store
        .read(move |c| {
            let row = c
                .query_row(
                    "SELECT owned_keys_json,last_op,backup_path,existed_at_first_write
                       FROM client_config_state WHERE client=?1 AND file_path=?2",
                    params![client, rel],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, i64>(3)?,
                        ))
                    },
                )
                .optional()?;
            match row {
                None => Ok(None),
                Some((json, last_op, backup, existed)) => Ok(Some(Record {
                    snapshot: Snapshot::from_json(&json)?,
                    last_op,
                    backup_path: backup.map(PathBuf::from),
                    existed_at_first_write: existed != 0,
                })),
            }
        })
        .await
}

/// 写一行（upsert）。**首写快照路径与「首写时文件是否存在」一旦记下就不再改**（COALESCE / 保留原值）。
pub(crate) async fn save(
    store: &Store,
    client: &str,
    rel: &str,
    snapshot: &Snapshot,
    last_op: &str,
    backup_path: Option<&Path>,
    existed: bool,
) -> Result<()> {
    let client = client.to_string();
    let rel = rel.to_string();
    let json = snapshot.to_json();
    let last_op = last_op.to_string();
    let backup = backup_path.map(|p| p.to_string_lossy().to_string());
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    store
        .write(move |c| {
            c.execute(
                "INSERT INTO client_config_state
                     (client,file_path,owned_keys_json,last_op,last_write_at,backup_path,existed_at_first_write)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(client,file_path) DO UPDATE SET
                     owned_keys_json=excluded.owned_keys_json,
                     last_op=excluded.last_op,
                     last_write_at=excluded.last_write_at,
                     backup_path=COALESCE(client_config_state.backup_path, excluded.backup_path),
                     existed_at_first_write=client_config_state.existed_at_first_write",
                params![client, rel, json, last_op, at, backup, existed as i64],
            )?;
            Ok(())
        })
        .await
}
