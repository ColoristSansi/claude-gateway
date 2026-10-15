//! 模型发现（model discovery，spec §6 / §10.2；票 23 / 35 / 50）。
//!
//! [`provider`](crate::egress::provider) 给的是**原语**（`pull_models` 打一次 `/models`、
//! `apply_pull` 把结果灌进目录、`save_manual_model` 手工条目）；本模块把原语拼成用户按一次
//! 「拉模型」时真正发生的那件事，并把**三种结局的分流与降级处置**成界面能直接渲染的
//! [`PullReport`]（票 52 拿它画结果行、提示文案与按钮）。
//!
//! # 三种结局（票 35 第 2 / 4 条）
//!
//! | 结局 | 判据 | 处置 |
//! | --- | --- | --- |
//! | [`PullDisposition::Fetched`] | 2xx 且有条目 | 目录入库（成功才覆盖 `upstream` / `doc` 条目并清角标） |
//! | [`PullDisposition::NoDiscovery`] | 404 / 405 / 200 零条 / 声明了 `NoModelsEndpoint` | 软提示「请手填模型 ID」+ 手填入口，**不报红**、不动目录 |
//! | [`PullDisposition::Failed`] | 其他非 2xx / 超时 / 断网 | 失败提示 + 内联「手工添加模型」+「重试」；**保留旧条目**，上游仍可保存 |
//!
//! 拉取之前还有两道门，它们同样要界面出手，所以也算本类型的一部分：
//! [`PullDisposition::NoCredential`]（该上游的清单只认带凭据那份，没凭据就不打）与
//! [`PullDisposition::CredentialChoice`]（多份凭据又没有 `default`，放下拉让用户选一份）。
//!
//! # 不阻断、不计入就绪
//!
//! 拉模型**从不阻断保存**（[`run_pull`] 回回执而不是 `Err`；失败也只落 `last_pull_status`），
//! 也不进概览「最近错误」（那是 `requests` 的地盘）。侧栏「未通过自检的上游数」与概览三步
//! 就绪**只看自检**（[`upstream_tally`]，票 35 第 7 条）：角标与失败只影响
//! [`UpstreamTally::with_pull_warning`]。
//!
//! # 重试口径
//!
//! 一次 [`run_pull`] 至多打**两次**网络：先是带凭据那次；只有「该上游的清单只认带凭据那份」
//! （[`Quirk::ModelsListNeedsCredential`]）且带凭据那次 404 / 零条时，才**退匿名**再打一次
//! （票 35 第 8 条）。这不是自动重试——重试按钮在界面上（票 35 第 9 条）。

use anyhow::{anyhow, Result};

use crate::egress::provider::{
    apply_pull, carried_pull_facts, credential_wires, list_upstreams, pull_credential, pull_models,
    save_upstream, PullApplied, PullOutcome, Quirk, Upstream, MODELS_PATH,
};
use crate::store::Store;

/// 一次「拉模型」的结局（前三支是票 35 的三支；后两支是拉取之前的两道凭据门）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullDisposition {
    Fetched,
    NoDiscovery,
    Failed,
    /// 该上游的清单只认带凭据那份，而库里一份凭据都没有：不拉，先填 key 或手填模型 ID。
    NoCredential,
    /// 多份凭据且没有 `default`：界面下拉选一份再拉。
    CredentialChoice,
}

/// 界面上的按钮（票 52 按顺序渲染；文案由 [`PullAction::label`] 给，省得两边各写一套）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullAction {
    AddManualModel,
    Retry,
    FillCredential,
    ChooseCredential,
}

impl PullAction {
    pub fn label(&self) -> &'static str {
        match self {
            PullAction::AddManualModel => "手工添加模型",
            PullAction::Retry => "重试",
            PullAction::FillCredential => "去填 key",
            PullAction::ChooseCredential => "选一份凭据",
        }
    }
}

/// 这次目录是从哪份凭据拉的（界面要如实说，尤其退匿名那次）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullCredential {
    /// 用了上游凭据（`default` 或唯一一份）。
    Credentialed,
    /// 带凭据那次拉不到清单，退了匿名兜底：目录来自**未授权**的全量列表。
    AnonymousFallback,
    /// 本来就没有凭据，尽力而为地匿名拉（可用性靠自检）。
    Anonymous,
    /// 没打网络（声明了不必尝试 / 没凭据 / 等界面选凭据）。
    NotAttempted,
}

impl PullCredential {
    /// 界面提示；正常路径（带凭据）不需要额外标注。
    pub fn note(&self) -> Option<&'static str> {
        match self {
            PullCredential::Credentialed => None,
            PullCredential::AnonymousFallback => Some(
                "带凭据没拉到清单，已退匿名：这份目录是未授权的全量列表，\
                 未授权的模型调用会 403，可用性靠自检",
            ),
            PullCredential::Anonymous => Some("没有凭据，匿名拉的目录：可用性靠自检"),
            PullCredential::NotAttempted => None,
        }
    }
}

/// 一次「拉模型」的回执（票 52 直接拿它渲染结果行、提示与按钮）。
#[derive(Debug, Clone, PartialEq)]
pub struct PullReport {
    pub upstream_id: String,
    /// 这次尝试的时间（Unix 秒）。
    pub at: i64,
    pub disposition: PullDisposition,
    /// 结果行 / 提示的文案（人话）。
    pub message: String,
    pub credential: PullCredential,
    /// 这次目录动了什么（只有成功那支有计数）。
    pub applied: PullApplied,
    /// 界面该给的按钮（顺序即渲染顺序）。
    pub actions: Vec<PullAction>,
    /// 这次没换目录、而以前成功过：提示「沿用上次结果（<这个时间>）」，界面按本地时区格式化。
    pub reuse_at: Option<i64>,
    /// 本次拉取留下的 wire 反查警告（内联一行一条）。
    pub warnings: Vec<String>,
    /// 多份凭据待选时给出可选的 wire 名（[`PullDisposition::CredentialChoice`]）。
    pub choose_from: Vec<String>,
}

impl PullReport {
    pub fn offers(&self, action: PullAction) -> bool {
        self.actions.contains(&action)
    }
}

/// 拉一次模型：解析凭据 → 打 `/models`（必要时退匿名）→ 把结果灌进目录 → 给界面回执。
///
/// 声明会先入库（[`save_upstream`]）：向导的自然顺序是「填 baseUrl 与 key → 拉模型 → 强制自检并
/// 保存」，而目录条目有外键、必须挂在已存在的上游行上。这一步只写声明式字段，不碰运行期事实。
pub async fn run_pull(
    store: &Store,
    http: &reqwest::Client,
    up: &Upstream,
    at: i64,
) -> Result<PullReport> {
    save_upstream(store, up).await?;
    // 上次成功拉取的时间：这次不换目录时，卡片要报「沿用上次结果（时间）」。
    let previous_ok_at = carried_pull_facts(store, &up.id).await?.0;

    // 声明了「上游根本不提供模型发现」：不必尝试（spec §6 收窄后的 NoModelsEndpoint）。
    if up.has_quirk(&Quirk::NoModelsEndpoint) {
        let applied = apply_pull(store, up, PullOutcome::NoDiscovery, at).await?;
        return Ok(report(
            up,
            at,
            previous_ok_at,
            Verdict {
                disposition: PullDisposition::NoDiscovery,
                credential: PullCredential::NotAttempted,
                message: "该上游声明不提供模型发现（noModelsEndpoint），已跳过拉取：请手填模型 ID"
                    .into(),
                actions: vec![PullAction::AddManualModel],
                applied,
            },
        ));
    }

    let wires = credential_wires(store, &up.id).await?;
    let secret = pull_credential(store, &up.id).await?;
    let (outcome, via) = match (&secret, wires.is_empty()) {
        // 该上游的清单只认带凭据那份，而一份都没有：不拉（票 35 第 8 条）。
        (None, true) if up.has_quirk(&Quirk::ModelsListNeedsCredential) => {
            return Ok(report(
                up,
                at,
                previous_ok_at,
                Verdict {
                    disposition: PullDisposition::NoCredential,
                    credential: PullCredential::NotAttempted,
                    message:
                        "该上游的模型清单只认带凭据那份（没凭据不拉）：先填 key，或手填模型 ID"
                            .into(),
                    actions: vec![PullAction::FillCredential, PullAction::AddManualModel],
                    applied: PullApplied::default(),
                },
            ));
        }
        // 多份凭据又没有 default：不瞎猜，交给界面下拉选（spec §6）。
        (None, false) => {
            let mut r = report(
                up,
                at,
                previous_ok_at,
                Verdict {
                    disposition: PullDisposition::CredentialChoice,
                    credential: PullCredential::NotAttempted,
                    message: "该上游有多份凭据且没有 default：选一份再拉，或手填模型 ID".into(),
                    actions: vec![PullAction::ChooseCredential, PullAction::AddManualModel],
                    applied: PullApplied::default(),
                },
            );
            r.choose_from = wires;
            return Ok(r);
        }
        (Some(key), _) => {
            let outcome = pull_models(http, up, MODELS_PATH, Some(key)).await;
            match (&outcome, up.has_quirk(&Quirk::ModelsListNeedsCredential)) {
                // 带凭据 404 / 零条才退匿名（票 35 第 8 条）：opencode 那种「两份语义不同的目录」。
                (PullOutcome::NoDiscovery, true) => (
                    pull_models(http, up, MODELS_PATH, None).await,
                    PullCredential::AnonymousFallback,
                ),
                _ => (outcome, PullCredential::Credentialed),
            }
        }
        (None, true) => (
            pull_models(http, up, MODELS_PATH, None).await,
            PullCredential::Anonymous,
        ),
    };

    let disposition = match &outcome {
        PullOutcome::Fetched(_) => PullDisposition::Fetched,
        PullOutcome::NoDiscovery => PullDisposition::NoDiscovery,
        PullOutcome::Failed(_) => PullDisposition::Failed,
    };
    // 「从哪份凭据拉的」只在真的拉到目录时才有意义：没拉到时界面不该说「这份目录是匿名的」。
    let credential = match disposition {
        PullDisposition::Fetched => via,
        _ => PullCredential::NotAttempted,
    };
    let actions: Vec<PullAction> = match disposition {
        PullDisposition::Fetched | PullDisposition::NoDiscovery => vec![PullAction::AddManualModel],
        _ => vec![PullAction::AddManualModel, PullAction::Retry],
    };
    let failure = match &outcome {
        PullOutcome::Failed(why) => Some(why.clone()),
        _ => None,
    };

    let applied = apply_pull(store, up, outcome, at).await?;
    let message = match disposition {
        PullDisposition::Fetched => format!(
            "拉到 {} 条（新增 {} / 更新 {} / 清掉 {} / 手改锁定 {}）",
            applied.fetched, applied.added, applied.updated, applied.removed, applied.kept_manual
        ),
        PullDisposition::NoDiscovery => {
            "该上游不提供模型发现（404/405 或 200 零条）：请手填模型 ID".to_string()
        }
        PullDisposition::Failed => failure.unwrap_or_else(|| "拉取失败".to_string()),
        // 拉取前的两道凭据门在上面就返回了。
        PullDisposition::NoCredential | PullDisposition::CredentialChoice => {
            return Err(anyhow!("凭据门不该走到这里"))
        }
    };
    Ok(report(
        up,
        at,
        previous_ok_at,
        Verdict {
            disposition,
            credential,
            message,
            actions,
            applied,
        },
    ))
}

/// 侧栏与概览要的两个数（票 35 第 7 条：判据只看自检）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UpstreamTally {
    pub total: usize,
    /// 上次自检通过的上游数（`last_selfcheck_status = 'ok'`）。
    pub selfcheck_passed: usize,
    /// 有告警角标（「上次拉取警告」非空）的上游数——只给界面点角标用。
    pub with_pull_warning: usize,
}

impl UpstreamTally {
    /// 侧栏「未通过自检的上游数」：**没自检过也算未通过**。
    /// 拉取失败与 wire 反查警告都不进这个数（spec §6 / 票 35 第 7 条）。
    pub fn selfcheck_failed(&self) -> usize {
        self.total - self.selfcheck_passed
    }

    /// 概览三步第一步「上游就绪」= 至少一条已保存且自检通过。
    pub fn ready(&self) -> bool {
        self.selfcheck_passed > 0
    }
}

/// 数一遍上游：自检通过数（就绪判据）与告警角标数（纯展示）。
pub async fn upstream_tally(store: &Store) -> Result<UpstreamTally> {
    let ups = list_upstreams(store).await?;
    let mut tally = UpstreamTally {
        total: ups.len(),
        ..Default::default()
    };
    for up in &ups {
        if up.last_selfcheck.map(|s| s.ok) == Some(true) {
            tally.selfcheck_passed += 1;
        }
        if up
            .last_pull
            .as_ref()
            .and_then(|p| p.warning.as_deref())
            .is_some_and(|w| !w.trim().is_empty())
        {
            tally.with_pull_warning += 1;
        }
    }
    Ok(tally)
}

/// 这次拉取的判决（回执里随结局变的那几项）。
struct Verdict {
    disposition: PullDisposition,
    credential: PullCredential,
    message: String,
    actions: Vec<PullAction>,
    applied: PullApplied,
}

fn report(up: &Upstream, at: i64, previous_ok_at: Option<i64>, v: Verdict) -> PullReport {
    PullReport {
        upstream_id: up.id.clone(),
        at,
        disposition: v.disposition,
        message: v.message,
        credential: v.credential,
        // 只有真的换了目录才不叫「沿用上次结果」。
        reuse_at: match v.disposition {
            PullDisposition::Fetched => None,
            _ => previous_ok_at,
        },
        warnings: v.applied.warnings.clone(),
        applied: v.applied,
        actions: v.actions,
        choose_from: Vec::new(),
    }
}
