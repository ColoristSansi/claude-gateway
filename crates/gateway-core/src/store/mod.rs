//! SQLite 持久化（spec §9）：建表与迁移、连接策略、DPAPI 凭据、数据目录。
//!
//! **连接策略 = 单写线程 + 请求排队**（票 42 的二选一）。原型那笔债是「每行单开连接 +
//! 同步写库阻塞 runtime」（`.scratch/model-gateway/prototype/src/store.rs` 的 `log()`，
//! [ADR-0007] 第 6 条）；这里两处都改掉：
//!
//! - 写只有一个入口 [`Store::write`]：闭包打包成 job 发给专属写线程，调用方只 `await`
//!   一个 oneshot —— **写库不在 runtime 线程上跑**，也不再每行开连接。
//! - 读走 [`Store::read`]：一条常驻读连接 + `spawn_blocking`（WAL 下读写不互斥）。
//! - 两条路都回 `Result`：写线程没了就报错，不静默丢日志。
//!
//! 为什么不选连接池：库在本地、单进程、写点只有请求日志与配置两类。连接池要多维护尺寸 /
//! 超时 / 回收三件事；单写线程把并发写天然变成排队，顺序即提交顺序（日志写序＝请求发生序），
//! 读并发用 WAL 的读写不互斥就够。真到读扛不住再换池，`read` 是唯一要动的地方。
//!
//! [ADR-0007]: ../../../docs/adr/0007-skeleton-and-implementation-order.md

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};

mod dpapi;
mod schema;
#[cfg(test)]
mod tests;

pub use schema::SCHEMA_VERSION;

/// 数据目录名 = Tauri identifier（票 18）：`%APPDATA%\com.claudegateway.desktop\`。
pub const DATA_DIR_NAME: &str = "com.claudegateway.desktop";
/// 库文件名（票 18）。
pub(crate) const DB_FILE_NAME: &str = "gateway.db";

/// `settings` 的键（spec §9：端口、日志保留、汇率、快照刷新时间）。
pub mod settings_keys {
    pub const PORT: &str = "port";
    pub const LOG_RETENTION_DAYS: &str = "log_retention_days";
    pub const USD_CNY_RATE: &str = "usd_cny_rate";
    pub const CATALOG_SNAPSHOT_AT: &str = "catalog_snapshot_at";
}

/// 数据目录（票 18）。**只算路径、不创建**——创建发生在 [`Store::open`]。
pub fn default_data_dir() -> Result<PathBuf> {
    let appdata =
        std::env::var_os("APPDATA").ok_or_else(|| anyhow!("APPDATA 未设置，无法定位数据目录"))?;
    Ok(PathBuf::from(appdata).join(DATA_DIR_NAME))
}

/// 界面侧只看到掩码（spec §9）：留前 3 后 4；短于 9 位整串掩掉，不泄露长度。
pub fn mask_secret(secret: &str) -> String {
    let n = secret.chars().count();
    if n <= 8 {
        return "••••".to_string();
    }
    let head: String = secret.chars().take(3).collect();
    let tail: String = secret.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// 发给写线程的一个 job：闭包在写线程上跑，结果经 oneshot 回给调用方。
type WriteJob = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// 存储句柄：可克隆，克隆体共用同一条写队列与同一条读连接。
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    db_path: PathBuf,
    write_tx: mpsc::UnboundedSender<WriteJob>,
    reader: Arc<Mutex<Connection>>,
}

impl Store {
    /// 打开（必要时首次自建数据目录与库）并升到最新 schema。
    ///
    /// 建目录 / 迁移是**启动期一次性**的同步动作，不在请求路径上。
    pub fn open(root: &Path) -> Result<Store> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("建数据目录失败：{}", root.display()))?;
        let db_path = root.join(DB_FILE_NAME);

        let mut conn = Connection::open(&db_path)
            .with_context(|| format!("开库失败：{}", db_path.display()))?;
        tune(&conn)?;
        schema::migrate(&mut conn)?;

        // 读连接：WAL 下与写线程并行，不再每行开连接。
        let reader = Connection::open(&db_path)
            .with_context(|| format!("开读连接失败：{}", db_path.display()))?;
        tune(&reader)?;

        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<WriteJob>();
        std::thread::Builder::new()
            .name("gateway-store-write".into())
            .spawn(move || {
                // shortcut: 队列无界——本地 sqlite 写线程不会落后，宁可涨一点内存也不阻塞请求路径；
                // 真出现积压再加容量与丢弃策略。
                while let Some(job) = write_rx.blocking_recv() {
                    job(&mut conn);
                }
            })
            .context("起存储写线程失败")?;

        Ok(Store {
            root: root.to_path_buf(),
            db_path,
            write_tx,
            reader: Arc::new(Mutex::new(reader)),
        })
    }

    /// `%APPDATA%\com.claudegateway.desktop\` 上的那份库（外壳用）。
    pub fn open_default() -> Result<Store> {
        Store::open(&default_data_dir()?)
    }

    /// 数据目录（`backups\` / `logs\` / `updates\` 都挂在它下面，票 18）。
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 写：闭包在专属写线程上执行。`await` 回来时事务已提交。
    pub async fn write<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    {
        let (done_tx, done_rx) = oneshot::channel();
        self.write_tx
            .send(Box::new(move |c| {
                let _ = done_tx.send(f(c));
            }))
            .map_err(|_| anyhow!("存储写线程已退出"))?;
        done_rx
            .await
            .map_err(|_| anyhow!("存储写线程未回执就退出"))?
    }

    /// 读：在 blocking 池上跑，不占 runtime 线程。
    pub async fn read<T, F>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
    {
        let reader = self.reader.clone();
        tokio::task::spawn_blocking(move || {
            let conn = reader.lock().unwrap_or_else(|e| e.into_inner());
            f(&conn)
        })
        .await
        .context("读任务被取消")?
    }

    /// 写一份凭据（按「上游 × wire」）：**只有 DPAPI 密文入库**（spec §9）。
    pub async fn put_credential(&self, upstream_id: &str, wire: &str, secret: &str) -> Result<()> {
        let (upstream_id, wire, secret) = (
            upstream_id.to_string(),
            wire.to_string(),
            secret.to_string(),
        );
        self.write(move |c| {
            let cipher = dpapi::protect(secret.as_bytes())?;
            c.execute(
                "INSERT INTO credentials(upstream_id,wire,ciphertext) VALUES(?1,?2,?3)
                 ON CONFLICT(upstream_id,wire) DO UPDATE SET ciphertext=excluded.ciphertext",
                params![upstream_id, wire, cipher],
            )?;
            Ok(())
        })
        .await
    }

    /// 取凭据明文（出站注入认证头用）。没有这一份 wire 的凭据 → `None`。
    pub async fn credential_secret(&self, upstream_id: &str, wire: &str) -> Result<Option<String>> {
        let (upstream_id, wire) = (upstream_id.to_string(), wire.to_string());
        self.read(move |c| {
            let mut st =
                c.prepare("SELECT ciphertext FROM credentials WHERE upstream_id=?1 AND wire=?2")?;
            let mut rows = st.query(params![upstream_id, wire])?;
            let cipher: Option<Vec<u8>> = match rows.next()? {
                Some(r) => Some(r.get(0)?),
                None => None,
            };
            match cipher {
                None => Ok(None),
                Some(b) => {
                    let plain = dpapi::unprotect(&b)?;
                    Ok(Some(
                        String::from_utf8(plain).context("凭据明文不是 UTF-8")?,
                    ))
                }
            }
        })
        .await
    }

    /// 界面侧的掩码读取（spec §9「界面只显示掩码」）：明文不出存储边界。
    pub async fn credential_mask(&self, upstream_id: &str, wire: &str) -> Result<Option<String>> {
        Ok(self
            .credential_secret(upstream_id, wire)
            .await?
            .map(|s| mask_secret(&s)))
    }

    pub async fn delete_credential(&self, upstream_id: &str, wire: &str) -> Result<()> {
        let (upstream_id, wire) = (upstream_id.to_string(), wire.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM credentials WHERE upstream_id=?1 AND wire=?2",
                params![upstream_id, wire],
            )?;
            Ok(())
        })
        .await
    }
}

/// 两条连接共用的 pragma：WAL（读写不互斥）+ 外键 + 短暂 busy 退让。
fn tune(c: &Connection) -> Result<()> {
    c.query_row("PRAGMA journal_mode=WAL", [], |r| r.get::<_, String>(0))
        .context("切 WAL 失败")?;
    c.execute_batch("PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000; PRAGMA synchronous=NORMAL;")
        .context("设连接 pragma 失败")?;
    Ok(())
}
