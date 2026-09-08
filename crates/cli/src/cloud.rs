//! 云平台（Vercel / Cloudflare）API 客户端与自动化管理。
//!
//! 提供基于 API Token 的一键自省、项目创建、环境变量注入、域名绑定与本地状态更新。

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

// =========================================================================
// Vercel 模型与客户端
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VercelUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VercelTeam {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VercelAccount {
    pub user: VercelUser,
    pub teams: Vec<VercelTeam>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VercelProject {
    pub id: String,
    pub name: String,
    #[serde(rename = "accountId", default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainVerificationRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub domain: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainStatus {
    Verified { domain: String },
    NeedsVerification { domain: String, records: Vec<DomainVerificationRecord> },
    AlreadyAssigned { domain: String },
}

#[derive(Debug, Clone)]
pub struct VercelClient {
    token: String,
    team_id: Option<String>,
    api_base: String,
}

impl VercelClient {
    pub fn new(token: impl Into<String>, team_id: Option<String>) -> Self {
        Self {
            token: token.into(),
            team_id,
            api_base: "https://api.vercel.com".to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_base(mut self, base: String) -> Self {
        self.api_base = base;
        self
    }

    pub fn team_id(&self) -> Option<&str> {
        self.team_id.as_deref()
    }

    fn url(&self, path: &str) -> String {
        let mut u = format!("{}{path}", self.api_base);
        if let Some(tid) = &self.team_id {
            if u.contains('?') {
                u.push_str(&format!("&teamId={tid}"));
            } else {
                u.push_str(&format!("?teamId={tid}"));
            }
        }
        u
    }

    fn http_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .no_proxy()
            .build()
            .unwrap_or_default()
    }

    /// 获取当前账户信息（用户 + 所有隶属团队）
    pub fn get_account_info(&self) -> Result<VercelAccount, String> {
        let client = Self::http_client();

        // 1. 获取用户信息
        let user_url = format!("{}/v2/user", self.api_base);
        let user_resp = client
            .get(&user_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .map_err(|e| format!("Vercel 获取用户信息网络错误: {e}"))?;

        if !user_resp.status().is_success() {
            let st = user_resp.status();
            let body = user_resp.text().unwrap_or_default();
            return Err(format!("Vercel 令牌验证失败 ({st}): {body}"));
        }

        #[derive(Deserialize)]
        struct UserResp {
            user: VercelUser,
        }
        let user_obj: UserResp = user_resp
            .json()
            .map_err(|e| format!("解析 Vercel 用户响应失败: {e}"))?;

        // 2. 获取团队列表
        let teams_url = format!("{}/v2/teams", self.api_base);
        let teams_resp = client
            .get(&teams_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .map_err(|e| format!("Vercel 获取团队信息网络错误: {e}"))?;

        let mut teams = Vec::new();
        if teams_resp.status().is_success() {
            #[derive(Deserialize)]
            struct TeamsResp {
                teams: Vec<VercelTeam>,
            }
            if let Ok(parsed) = teams_resp.json::<TeamsResp>() {
                teams = parsed.teams;
            }
        }

        Ok(VercelAccount {
            user: user_obj.user,
            teams,
        })
    }

    /// 幂等确保项目存在（若不存在则创建，若存在则返回已有项目）
    pub fn ensure_project(&self, name: &str) -> Result<VercelProject, String> {
        let client = Self::http_client();

        // 1. 先查询项目是否存在
        let get_url = self.url(&format!("/v9/projects/{name}"));
        let get_resp = client
            .get(&get_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .map_err(|e| format!("查询 Vercel 项目网络错误: {e}"))?;

        if get_resp.status().is_success() {
            let project: VercelProject = get_resp
                .json()
                .map_err(|e| format!("解析 Vercel 项目信息失败: {e}"))?;
            return Ok(project);
        }

        // 2. 不存在时创建
        let post_url = self.url("/v9/projects");
        let body = serde_json::json!({
            "name": name,
            "framework": null
        });

        let post_resp = client
            .post(&post_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .map_err(|e| format!("创建 Vercel 项目网络错误: {e}"))?;

        if post_resp.status().is_success() {
            let project: VercelProject = post_resp
                .json()
                .map_err(|e| format!("解析新建 Vercel 项目响应失败: {e}"))?;
            return Ok(project);
        }

        let st = post_resp.status();
        let err_body = post_resp.text().unwrap_or_default();
        Err(format!("创建 Vercel 项目 '{name}' 失败 ({st}): {err_body}"))
    }

    /// 幂等设置环境变量
    pub fn set_env_var(&self, project_id: &str, key: &str, value: &str) -> Result<(), String> {
        let client = Self::http_client();
        let url = self.url(&format!("/v10/projects/{project_id}/env"));

        let body = serde_json::json!({
            "key": key,
            "value": value,
            "type": "encrypted",
            "target": ["production", "preview", "development"]
        });

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .map_err(|e| format!("设置环境变量网络错误: {e}"))?;

        let status = resp.status();
        if status.is_success() || status.as_u16() == 409 {
            // 200/201 成功；409 表示已存在同名键，尝试更新现有键
            if status.as_u16() == 409 {
                return self.update_existing_env_var(project_id, key, value);
            }
            return Ok(());
        }

        let err_body = resp.text().unwrap_or_default();
        // 部分 Vercel 错误码会把 key 冲突当作 400 返回
        if err_body.contains("already exists") || err_body.contains("ENV_ALREADY_EXISTS") {
            return self.update_existing_env_var(project_id, key, value);
        }

        Err(format!("设置 Vercel 环境变量 '{key}' 失败 ({status}): {err_body}"))
    }

    /// 查找并更新现有环境变量
    fn update_existing_env_var(&self, project_id: &str, key: &str, value: &str) -> Result<(), String> {
        let client = Self::http_client();
        let list_url = self.url(&format!("/v9/projects/{project_id}/env"));

        let list_resp = client
            .get(&list_url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .map_err(|e| format!("获取环境变量列表失败: {e}"))?;

        if !list_resp.status().is_success() {
            return Err("无法检索现有环境变量以进行覆盖更新".into());
        }

        #[derive(Deserialize)]
        struct EnvItem {
            id: String,
            key: String,
        }
        #[derive(Deserialize)]
        struct ListResp {
            envs: Vec<EnvItem>,
        }

        let env_list = list_resp.json::<ListResp>().map_err(|e| e.to_string())?;
        if let Some(target) = env_list.envs.into_iter().find(|e| e.key == key) {
            let patch_url = self.url(&format!("/v10/projects/{project_id}/env/{}", target.id));
            let patch_body = serde_json::json!({
                "value": value,
                "target": ["production", "preview", "development"]
            });
            let patch_resp = client
                .patch(&patch_url)
                .header("Authorization", format!("Bearer {}", self.token))
                .json(&patch_body)
                .send()
                .map_err(|e| format!("更新环境变量网络错误: {e}"))?;

            if patch_resp.status().is_success() {
                return Ok(());
            }
            return Err(format!("更新环境变量失败: {}", patch_resp.text().unwrap_or_default()));
        }

        Ok(())
    }

    /// 幂等绑定域名并返回验证要求
    pub fn ensure_domain(&self, project_id: &str, domain: &str) -> Result<DomainStatus, String> {
        let client = Self::http_client();
        let url = self.url(&format!("/v9/projects/{project_id}/domains"));

        let body = serde_json::json!({
            "name": domain
        });

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .map_err(|e| format!("绑定域名网络错误: {e}"))?;

        let status = resp.status();
        let text = resp.text().unwrap_or_default();

        #[derive(Deserialize)]
        struct DomainResp {
            #[serde(default)]
            verified: Option<bool>,
            #[serde(default)]
            verification: Option<Vec<DomainVerificationRecord>>,
        }

        if status.is_success() {
            if let Ok(d) = serde_json::from_str::<DomainResp>(&text) {
                if d.verified.unwrap_or(false) {
                    return Ok(DomainStatus::Verified { domain: domain.to_string() });
                }
                if let Some(recs) = d.verification {
                    if !recs.is_empty() {
                        return Ok(DomainStatus::NeedsVerification {
                            domain: domain.to_string(),
                            records: recs,
                        });
                    }
                }
            }
            return Ok(DomainStatus::Verified { domain: domain.to_string() });
        }

        if status.as_u16() == 409 {
            // 域名已在此项目或其它项目中
            return Ok(DomainStatus::AlreadyAssigned { domain: domain.to_string() });
        }

        Err(format!("添加域名 '{domain}' 失败 ({status}): {text}"))
    }
}

/// 将 projectId / orgId 写入目标工作目录的 `.vercel/project.json`
pub fn write_local_project_json(
    work_dir: &Path,
    project_id: &str,
    org_id: &str,
    project_name: &str,
) -> Result<PathBuf, String> {
    let dot_vercel = work_dir.join(".vercel");
    if !dot_vercel.exists() {
        std::fs::create_dir_all(&dot_vercel)
            .map_err(|e| format!("创建 .vercel 目录失败 ({}): {e}", dot_vercel.display()))?;
    }

    let target_file = dot_vercel.join("project.json");
    let json_val = serde_json::json!({
        "projectId": project_id,
        "orgId": org_id,
        "projectName": project_name
    });

    let s = serde_json::to_string_pretty(&json_val).map_err(|e| e.to_string())?;
    std::fs::write(&target_file, s)
        .map_err(|e| format!("写入 project.json 失败 ({}): {e}", target_file.display()))?;

    Ok(target_file)
}

// =========================================================================
// Cloudflare 模型与客户端
// =========================================================================

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloudflareAccount {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct CloudflareClient {
    token: String,
    api_base: String,
}

impl CloudflareClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            api_base: "https://api.cloudflare.com/client/v4".to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_base(mut self, base: String) -> Self {
        self.api_base = base;
        self
    }

    fn http_client() -> reqwest::blocking::Client {
        reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .no_proxy()
            .build()
            .unwrap_or_default()
    }

    /// 自动列出用户有权限访问的 Cloudflare 账户
    pub fn list_accounts(&self) -> Result<Vec<CloudflareAccount>, String> {
        let client = Self::http_client();
        let url = format!("{}/accounts", self.api_base);

        let resp = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .map_err(|e| format!("Cloudflare 获取账户列表失败: {e}"))?;

        #[derive(Deserialize)]
        struct CfResponse {
            success: bool,
            #[serde(default)]
            result: Vec<CloudflareAccount>,
            #[serde(default)]
            errors: Vec<serde_json::Value>,
        }

        let st = resp.status();
        let text = resp.text().unwrap_or_default();
        if !st.is_success() {
            return Err(format!("Cloudflare API 错误 ({st}): {text}"));
        }

        let parsed: CfResponse = serde_json::from_str(&text)
            .map_err(|e| format!("解析 Cloudflare 账户列表失败: {e}"))?;

        if !parsed.success {
            return Err(format!("Cloudflare 验证失败: {:?}", parsed.errors));
        }

        Ok(parsed.result)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_vercel_url_construction() {
        let c1 = VercelClient::new("token1", None);
        assert_eq!(c1.url("/v9/projects"), "https://api.vercel.com/v9/projects");

        let c2 = VercelClient::new("token1", Some("team_abc123".into()));
        assert_eq!(
            c2.url("/v9/projects"),
            "https://api.vercel.com/v9/projects?teamId=team_abc123"
        );
        assert_eq!(
            c2.url("/v9/projects?limit=10"),
            "https://api.vercel.com/v9/projects?limit=10&teamId=team_abc123"
        );
    }

    #[test]
    fn test_parse_vercel_user_and_teams() {
        let user_json = r#"{
            "user": {
                "id": "usr_12345",
                "username": "ponydev",
                "email": "dev@example.com",
                "name": "Pony Developer"
            }
        }"#;
        #[derive(Deserialize)]
        struct UResp {
            user: VercelUser,
        }
        let u: UResp = serde_json::from_str(user_json).unwrap();
        assert_eq!(u.user.id, "usr_12345");
        assert_eq!(u.user.username, "ponydev");

        let teams_json = r#"{
            "teams": [
                {
                    "id": "team_111",
                    "slug": "team-one",
                    "name": "Team One"
                },
                {
                    "id": "team_222",
                    "slug": "team-two",
                    "name": "Team Two"
                }
            ]
        }"#;
        #[derive(Deserialize)]
        struct TResp {
            teams: Vec<VercelTeam>,
        }
        let t: TResp = serde_json::from_str(teams_json).unwrap();
        assert_eq!(t.teams.len(), 2);
        assert_eq!(t.teams[0].slug, "team-one");
        assert_eq!(t.teams[1].id, "team_222");
    }

    #[test]
    fn test_parse_domain_verification() {
        let domain_resp = r#"{
            "name": "vedge.ponyjob.top",
            "verified": false,
            "verification": [
                {
                    "type": "TXT",
                    "domain": "_vercel.vedge.ponyjob.top",
                    "value": "vc-domain-verify=abcdef"
                }
            ]
        }"#;

        #[derive(Deserialize)]
        struct DomainResp {
            verified: bool,
            verification: Vec<DomainVerificationRecord>,
        }
        let d: DomainResp = serde_json::from_str(domain_resp).unwrap();
        assert!(!d.verified);
        assert_eq!(d.verification.len(), 1);
        assert_eq!(d.verification[0].record_type, "TXT");
        assert_eq!(d.verification[0].domain, "_vercel.vedge.ponyjob.top");
        assert_eq!(d.verification[0].value, "vc-domain-verify=abcdef");
    }

    #[test]
    fn test_write_local_project_json() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_local_project_json(
            tmp.path(),
            "prj_test_123",
            "team_org_456",
            "my-cool-project",
        )
        .unwrap();

        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        let val: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(val["projectId"], "prj_test_123");
        assert_eq!(val["orgId"], "team_org_456");
        assert_eq!(val["projectName"], "my-cool-project");

        // 再次覆写
        let path2 = write_local_project_json(
            tmp.path(),
            "prj_test_updated",
            "team_org_456",
            "my-cool-project",
        )
        .unwrap();
        assert_eq!(path, path2);
        let content2 = std::fs::read_to_string(&path2).unwrap();
        let val2: serde_json::Value = serde_json::from_str(&content2).unwrap();
        assert_eq!(val2["projectId"], "prj_test_updated");
    }

    #[test]
    fn test_parse_cf_accounts() {
        let cf_json = r#"{
            "success": true,
            "errors": [],
            "messages": [],
            "result": [
                {
                    "id": "6f0f4f7f6dfe8ee82061083589fcb212",
                    "name": "Alice Account"
                }
            ]
        }"#;

        #[derive(Deserialize)]
        struct CfResponse {
            success: bool,
            result: Vec<CloudflareAccount>,
        }
        let parsed: CfResponse = serde_json::from_str(cf_json).unwrap();
        assert!(parsed.success);
        assert_eq!(parsed.result.len(), 1);
        assert_eq!(parsed.result[0].id, "6f0f4f7f6dfe8ee82061083589fcb212");
        assert_eq!(parsed.result[0].name, "Alice Account");
    }
}
