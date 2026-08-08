//! S-02：自定义服务商 URL 校验、SSRF 防护与响应资源上限。
//!
//! 三条网络路径（ASR / Chat / fetch_custom_models）共同使用本模块的
//! `validate_endpoint` 单一入口；请求连接前还需 `recheck_endpoint`
//! 重新解析 DNS（防 DNS rebinding），HTTP 客户端禁用自动重定向。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 模型列表成功正文上限（S-02 建议值）
pub const MODEL_LIST_BODY_LIMIT: u64 = 2 * 1024 * 1024;
/// 模型数量上限
pub const MAX_MODELS: usize = 10_000;
/// 单个模型 ID 长度上限（字符数）
pub const MAX_MODEL_ID_CHARS: usize = 256;
/// 错误正文保留上限（UTF-8 字符边界安全截断，复用 B-01 语义）
pub const ERROR_BODY_LIMIT: usize = 4 * 1024;

/// 端点安全策略。默认策略只允许公网 HTTPS；
/// 私网开发模式（allow_private_dev）仅放行 loopback HTTP，
/// 且该开关只能由用户在 UI 主动开启，不得由模型输出/事件自动开启。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EndpointPolicy {
    pub allow_private_dev: bool,
}

impl EndpointPolicy {
    pub const DEFAULT: EndpointPolicy = EndpointPolicy {
        allow_private_dev: false,
    };
    /// 私网开发模式：预留给后续 allow_private_endpoint 配置项（需 UI 显式风险确认），
    /// 当前产品路径一律使用 DEFAULT。
    #[allow(dead_code)]
    pub const DEV_PRIVATE: EndpointPolicy = EndpointPolicy {
        allow_private_dev: true,
    };
}

/// 校验通过后的端点描述。host_kind 记录 host 是 IP 字面量还是域名，
/// IP 字面量无需请求前重新解析 DNS，域名则必须 recheck。
#[derive(Debug)]
pub struct Endpoint {
    url: url::Url,
    host_kind: HostKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKind {
    Ip,
    Domain,
}

impl Endpoint {
    pub fn url(&self) -> &url::Url {
        &self.url
    }

    pub fn host_kind(&self) -> HostKind {
        self.host_kind
    }

    pub fn host(&self) -> &str {
        self.url.host_str().unwrap_or("")
    }

    /// 安全日志表示：只含 scheme/host/port/path（path 截断），
    /// 不含 userinfo、query 与任何凭据。
    pub fn safe_display(&self) -> String {
        let path = self.url.path();
        let path = if path.len() > 128 { &path[..128] } else { path };
        match self.url.port_or_known_default() {
            Some(port) => format!("{}://{}:{}{}", self.url.scheme(), self.host(), port, path),
            None => format!("{}://{}{}", self.url.scheme(), self.host(), path),
        }
    }

    /// 用 URL path API 拼接候选端点，禁止字符串拼接产生双斜杠或覆盖 origin。
    /// 行为约定：
    /// - path 已以 `/models` 结尾：按用户提供的完整端点使用（丢弃 query/fragment）；
    /// - path 已以 `/v1` 结尾：拼接 `/models`；
    /// - 其它：分别拼接 `/v1/models` 与 `/models` 两个候选。
    ///
    /// 尾部斜杠、查询参数在候选生成前统一去除。
    pub fn model_candidates(&self) -> Vec<url::Url> {
        let base = {
            let mut u = self.url.clone();
            u.set_query(None);
            u.set_fragment(None);
            let p = u.path().trim_end_matches('/').to_string();
            // 根路径归一为空串，避免后续拼接产生双斜杠
            u.set_path(if p.is_empty() { "/" } else { &p });
            u
        };
        let path = base.path().to_string();
        let lower = path.to_lowercase();
        // 根路径按空前缀处理，拼接结果形如 /v1/models 而非 //v1/models
        let prefix = if path == "/" { "" } else { path.as_str() };
        let mut out = Vec::new();
        if lower.ends_with("/models") {
            out.push(base);
        } else {
            let suffixes: &[&str] = if lower.ends_with("/v1") {
                &["/models"]
            } else {
                &["/v1/models", "/models"]
            };
            for s in suffixes {
                let mut u = base.clone();
                u.set_path(&format!("{}{}", prefix, s));
                out.push(u);
            }
        }
        out
    }
}

/// 端点校验错误（B-07：thiserror 统一错误体系）。消息为用户可理解的中文提示，
/// 不含 URL 全串/凭据；Display 文案与改造前一致。
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("服务地址无效: {0}")]
    InvalidUrl(String),
    #[error("服务地址被安全策略拒绝: {0}")]
    Restricted(String),
    #[error("服务地址解析失败: {0}")]
    Dns(String),
}

/// 单一校验入口（同步）：结构化解析 + scheme/host/端口/受限 IP 检查。
/// 域名 host 会执行 DNS 解析并检查全部 A/AAAA 结果（任一受限即整体拒绝）。
/// IP 字面量直接按最终 IP 判定，天然覆盖非标准表示（解析后归一）。
pub fn validate_endpoint(raw: &str, policy: EndpointPolicy) -> Result<Endpoint, EndpointError> {
    validate_endpoint_with(raw, policy, resolve_host)
}

/// 可注入 resolver 的校验实现（S-02 验收：不依赖真实公网 DNS 的用例使用可注入 resolver）
pub fn validate_endpoint_with<F>(
    raw: &str,
    policy: EndpointPolicy,
    resolver: F,
) -> Result<Endpoint, EndpointError>
where
    F: Fn(&str) -> std::io::Result<Vec<IpAddr>>,
{
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(EndpointError::InvalidUrl("不能为空".to_string()));
    }
    let url = url::Url::parse(trimmed)
        .map_err(|_| EndpointError::InvalidUrl("无法解析为合法 URL".to_string()))?;

    // userinfo（用户名/密码）一律禁止
    if !url.username().is_empty() || url.password().is_some() {
        return Err(EndpointError::Restricted(
            "URL 不允许包含用户名或密码".to_string(),
        ));
    }
    // fragment 无意义且可能干扰客户端行为，禁止
    if url.fragment().is_some() {
        return Err(EndpointError::InvalidUrl(
            "URL 不允许包含 fragment".to_string(),
        ));
    }
    // 端口：拒绝 0；显式端口必须是合法范围（url 解析已保证 <=65535）
    if let Some(port) = url.port() {
        if port == 0 {
            return Err(EndpointError::InvalidUrl("端口不能为 0".to_string()));
        }
    }

    let host_kind = match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            check_ip_restrictions(IpAddr::V4(ip), policy)?;
            HostKind::Ip
        }
        Some(url::Host::Ipv6(ip)) => {
            check_ip_restrictions(IpAddr::V6(ip), policy)?;
            HostKind::Ip
        }
        Some(url::Host::Domain(domain)) => {
            if domain.is_empty() {
                return Err(EndpointError::InvalidUrl("缺少主机名".to_string()));
            }
            check_domain_rules(domain, policy)?;
            // 对域名执行 DNS 解析，检查全部结果，不允许只挑一个公网结果
            let addrs = resolver(domain)
                .map_err(|e| EndpointError::Dns(format!("域名解析失败 ({})", e)))?;
            if addrs.is_empty() {
                return Err(EndpointError::Dns("域名未解析到任何地址".to_string()));
            }
            for ip in &addrs {
                check_ip_restrictions(*ip, policy)?;
            }
            HostKind::Domain
        }
        None => return Err(EndpointError::InvalidUrl("缺少主机名".to_string())),
    };

    // scheme 检查放在 IP 判定之后：私网开发模式仅允许 loopback 的 http
    match url.scheme() {
        "https" => {}
        "http" => {
            let is_loopback = url
                .host()
                .map(|h| match h {
                    url::Host::Ipv4(ip) => ip.is_loopback(),
                    url::Host::Ipv6(ip) => ip.is_loopback(), // ::1 同样豁免
                    url::Host::Domain(_) => false,           // 域名形式一律不放行明文 HTTP
                })
                .unwrap_or(false);
            if !(policy.allow_private_dev && is_loopback) {
                return Err(EndpointError::Restricted(
                    "仅允许 HTTPS 服务商地址".to_string(),
                ));
            }
        }
        other => {
            return Err(EndpointError::Restricted(format!(
                "不支持的协议: {}",
                other
            )));
        }
    }

    Ok(Endpoint { url, host_kind })
}

/// 域名形式规则：localhost 及其子域、全数字标签（八进制/十六进制/整数
/// 形式的 IP 绕过）等非标准表示一律拒绝；合法域名再走 DNS + IP 检查。
fn check_domain_rules(domain: &str, policy: EndpointPolicy) -> Result<(), EndpointError> {
    let lower = domain.to_lowercase();
    let trimmed = lower.trim_end_matches('.');
    if trimmed == "localhost" || trimmed.ends_with(".localhost") {
        return Err(EndpointError::Restricted("拒绝 localhost 地址".to_string()));
    }
    // 全数字标签可能是 2130706433 / 0x7f000001 等 IP 变体，
    // 交由 DNS 解析也可能被本地解析器当作 IP，直接拒绝最安全
    if trimmed
        .split('.')
        .any(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(EndpointError::Restricted(
            "拒绝非标准 IP 表示的主机名".to_string(),
        ));
    }
    // 私网开发模式只信任字面量 loopback IP，不放行任何域名形式
    let _ = policy;
    Ok(())
}

/// IP 受限判定。默认拒绝：loopback、private、link-local、unspecified、
/// multicast、broadcast、文档保留网段与云元数据地址；
/// IPv4-mapped IPv6 归一后检查。私网开发模式仅豁免 loopback。
/// 返回 Ok(()) 表示允许，Err 携带用户可读原因。
pub fn check_ip_restrictions(ip: IpAddr, policy: EndpointPolicy) -> Result<(), EndpointError> {
    let ip = match ip {
        IpAddr::V6(v6) => {
            // 只归一 IPv4-mapped（::ffff:a.b.c.d）；to_ipv4() 会把 ::1 误当
            // IPv4 兼容地址归一成 0.0.0.1，导致回环豁免失效
            if let Some(v4) = v6.to_ipv4_mapped() {
                IpAddr::V4(v4)
            } else {
                IpAddr::V6(v6)
            }
        }
        v4 @ IpAddr::V4(_) => v4,
    };

    let deny = |reason: &str| Err(EndpointError::Restricted(reason.to_string()));

    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return if policy.allow_private_dev {
                    Ok(())
                } else {
                    deny("拒绝回环地址")
                };
            }
            if v4.is_unspecified() {
                return deny("拒绝未指定地址");
            }
            if v4.is_private() || v4.is_link_local() {
                return deny("拒绝内网/链路本地地址");
            }
            if v4.is_broadcast() || v4.is_documentation() {
                return deny("拒绝广播/文档保留地址");
            }
            if v4.is_multicast() {
                return deny("拒绝组播地址");
            }
            // 云元数据 169.254.169.254 已被 link_local 覆盖，显式再拦一次
            if v4 == Ipv4Addr::new(169, 254, 169, 254) {
                return deny("拒绝云元数据地址");
            }
            // 0.0.0.0/8 中除 0.0.0.0 外的地址同样不可路由
            if v4.octets()[0] == 0 {
                return deny("拒绝未指定地址段");
            }
            Ok(())
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return if policy.allow_private_dev {
                    Ok(())
                } else {
                    deny("拒绝回环地址")
                };
            }
            if v6.is_unspecified() || v6.is_multicast() {
                return deny("拒绝未指定/组播地址");
            }
            // 链路本地（含 IPv6 元数据 fd00:ec2::254 所在网段）
            if is_ipv6_link_local(&v6) {
                return deny("拒绝链路本地地址");
            }
            // 唯一本地地址 fc00::/7（内网）
            if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                return deny("拒绝内网地址");
            }
            // IPv4 兼容地址 ::a.b.c.d（已弃用）按映射后的 IPv4 复检
            if let Some(v4) = v6.to_ipv4_mapped() {
                return check_ip_restrictions(IpAddr::V4(v4), policy);
            }
            // 6to4 / Teredo 隧道可能封装私网 IPv4，检查内嵌地址
            if let Some(inner) = tunnel_inner_v4(&v6) {
                return check_ip_restrictions(IpAddr::V4(inner), policy);
            }
            Ok(())
        }
    }
}

fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

/// 提取 6to4（2002::/16）与 Teredo（2001:0000::/32）隧道内嵌的 IPv4 地址
fn tunnel_inner_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let seg = v6.segments();
    if seg[0] == 0x2002 {
        Some(Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            seg[1] as u8,
            (seg[2] >> 8) as u8,
            seg[2] as u8,
        ))
    } else if seg[0] == 0x2001 && seg[1] == 0x0000 {
        // Teredo：内嵌 IPv4 位于 seg[6]/seg[7] 且按位取反
        Some(Ipv4Addr::new(
            !(seg[6] >> 8) as u8,
            !(seg[6] & 0xff) as u8,
            !(seg[7] >> 8) as u8,
            !(seg[7] & 0xff) as u8,
        ))
    } else {
        None
    }
}

/// DNS 解析（同步，std）。独立成函数便于检查逻辑复用；
/// 测试通过纯 IP 校验函数覆盖受限判定，不依赖真实公网 DNS。
pub fn resolve_host(host: &str) -> std::io::Result<Vec<IpAddr>> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<IpAddr> = (host, 0)
        .to_socket_addrs()?
        .map(|sa| sa.ip())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    Ok(addrs)
}

/// 防 DNS rebinding：每次真实请求前，对域名 host 重新解析并检查全部结果。
/// IP 字面量无需重查。配合禁用自动重定向使用。
pub async fn recheck_endpoint(endpoint: &Endpoint) -> Result<(), EndpointError> {
    if endpoint.host_kind() == HostKind::Ip {
        return Ok(());
    }
    let host = endpoint.host().to_string();
    let addrs = tokio::task::spawn_blocking(move || resolve_host(&host))
        .await
        .map_err(|_| EndpointError::Dns("域名解析任务异常".to_string()))?
        .map_err(|_| EndpointError::Dns("域名解析失败".to_string()))?;
    if addrs.is_empty() {
        return Err(EndpointError::Dns("域名未解析到任何地址".to_string()));
    }
    for ip in &addrs {
        check_ip_restrictions(*ip, EndpointPolicy::DEFAULT)?;
    }
    Ok(())
}

/// 按上限读取响应正文（S-02 响应资源限制）：超过 limit 立即终止并报错，
/// 防止恶意服务返回超大正文造成高内存占用。返回 UTF-8 宽容文本。
pub async fn read_body_limited(response: reqwest::Response, limit: u64) -> Result<String, String> {
    let mut resp = response;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        buf.extend_from_slice(&chunk);
        if buf.len() as u64 > limit {
            return Err(format!("响应正文超过 {} 字节上限", limit));
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// 按 UTF-8 字符边界安全保留错误正文（最多 `max_bytes` 字节），
/// 供错误消息复用，避免把超大响应正文带入错误/日志（S-02 + B-01 复用）。
pub fn safe_error_body_limited(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = 0usize;
    for (i, c) in text.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    let mut out = text[..end].to_string();
    out.push_str("…(已截断)");
    out
}

/// 按 UTF-8 字符边界安全保留错误正文（最多 ERROR_BODY_LIMIT 字节），
/// 供错误消息复用，避免把超大响应正文带入错误/日志（S-02 + B-01 复用）。
pub fn safe_error_body(text: &str) -> String {
    safe_error_body_limited(text, ERROR_BODY_LIMIT)
}

// ============================================================
// 单元测试（S-02 表驱动矩阵，不依赖真实公网 DNS）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;
    use std::str::FromStr;

    /// 可注入 resolver：所有域名解析到固定公网 IP，不依赖真实公网 DNS（S-02 验收要求）
    fn stub_public_resolver(_host: &str) -> std::io::Result<Vec<IpAddr>> {
        Ok(vec![IpAddr::from_str("93.184.216.34").unwrap()])
    }

    /// 混合解析结果：同一域名同时返回公网与私网地址
    fn stub_mixed_resolver(_host: &str) -> std::io::Result<Vec<IpAddr>> {
        Ok(vec![
            IpAddr::from_str("93.184.216.34").unwrap(),
            IpAddr::from_str("192.168.1.1").unwrap(),
        ])
    }

    fn ok(raw: &str) -> Endpoint {
        validate_endpoint_with(raw, EndpointPolicy::DEFAULT, stub_public_resolver)
            .expect("应通过校验")
    }

    fn denied(raw: &str) -> EndpointError {
        validate_endpoint(raw, EndpointPolicy::DEFAULT).expect_err("应被拒绝")
    }

    // ---------- 合法地址 ----------

    #[test]
    fn allows_standard_public_https() {
        let ep = ok("https://api.example.com/v1");
        assert_eq!(ep.url().scheme(), "https");
        assert_eq!(ep.host(), "api.example.com");
    }

    #[test]
    fn allows_explicit_443_and_path_variants() {
        ok("https://api.example.com:443/v1");
        ok("https://api.example.com/v1/");
        ok("https://api.example.com");
    }

    #[test]
    fn allows_public_ipv6() {
        // 2001:4860:4860::8888（Google 公共 DNS）为公网地址
        // 注意：该用例会触发真实 DNS 环境下的 IP 字面量校验（无 DNS 请求）
        ok("https://[2001:4860:4860::8888]/v1");
    }

    // ---------- 默认拒绝 ----------

    #[test]
    fn rejects_non_https_by_default() {
        assert!(matches!(
            denied("http://example.com"),
            EndpointError::Restricted(_)
        ));
        // file: 无主机名，解析阶段即拒绝（InvalidUrl 或 Restricted 均视为拒绝）
        assert!(matches!(
            denied("file:///C:/x"),
            EndpointError::InvalidUrl(_) | EndpointError::Restricted(_)
        ));
        assert!(matches!(
            denied("ftp://example.com"),
            EndpointError::Restricted(_)
        ));
        assert!(matches!(
            denied("data:text/plain,abc"),
            EndpointError::InvalidUrl(_) | EndpointError::Restricted(_)
        ));
    }

    #[test]
    fn rejects_missing_host_and_userinfo_and_port_zero() {
        assert!(validate_endpoint("", EndpointPolicy::DEFAULT).is_err());
        assert!(validate_endpoint("https://", EndpointPolicy::DEFAULT).is_err());
        assert!(matches!(
            denied("https://user:pass@example.com/v1"),
            EndpointError::Restricted(_)
        ));
        // url crate 解析时端口 0 会失败，同样必须拒绝
        assert!(validate_endpoint("https://example.com:0/v1", EndpointPolicy::DEFAULT).is_err());
    }

    #[test]
    fn rejects_restricted_ipv4_literals() {
        for addr in [
            "127.0.0.1",
            "127.0.0.2",
            "0.0.0.0",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "192.0.2.1", // 文档保留
            "255.255.255.255",
            "224.0.0.1", // 组播
        ] {
            let err = validate_endpoint(&format!("https://{}/v1", addr), EndpointPolicy::DEFAULT)
                .expect_err(&format!("{} 应被拒绝", addr));
            assert!(
                matches!(err, EndpointError::Restricted(_)),
                "{} => {:?}",
                addr,
                err
            );
        }
    }

    #[test]
    fn rejects_restricted_ipv6_literals() {
        for addr in [
            "[::1]",
            "[::]",
            "[fe80::1]",
            "[fc00::1]",
            "[ff02::1]",
            "[::ffff:192.168.1.1]",
            "[::ffff:127.0.0.1]",
        ] {
            let err = validate_endpoint(&format!("https://{}/v1", addr), EndpointPolicy::DEFAULT)
                .expect_err(&format!("{} 应被拒绝", addr));
            assert!(
                matches!(err, EndpointError::Restricted(_)),
                "{} => {:?}",
                addr,
                err
            );
        }
    }

    #[test]
    fn rejects_localhost_and_numeric_hostnames() {
        assert!(matches!(
            denied("https://localhost/v1"),
            EndpointError::Restricted(_)
        ));
        assert!(matches!(
            denied("https://sub.localhost/v1"),
            EndpointError::Restricted(_)
        ));
        // 纯整数/十六进制形式 IP 主机名
        assert!(matches!(
            denied("https://2130706433/v1"),
            EndpointError::Restricted(_)
        ));
        assert!(matches!(
            denied("https://0x7f000001/v1"),
            EndpointError::InvalidUrl(_) | EndpointError::Restricted(_)
        ));
    }

    // ---------- 私网开发模式 ----------

    #[test]
    fn dev_private_mode_allows_loopback_http_only() {
        let policy = EndpointPolicy::DEV_PRIVATE;
        let v = |raw: &str| validate_endpoint_with(raw, policy, stub_public_resolver);
        assert!(v("http://127.0.0.1:8080/v1").is_ok());
        assert!(v("http://[::1]:8080/v1").is_ok());
        // 私网地址即使开启也不放行（只豁免 loopback）
        assert!(v("http://192.168.1.1/v1").is_err());
        assert!(v("http://10.0.0.1/v1").is_err());
        // 元数据地址始终禁止
        assert!(v("http://169.254.169.254/v1").is_err());
        // 域名形式的 localhost 不放行明文 HTTP
        assert!(v("http://localhost:8080/v1").is_err());
        // loopback HTTPS 也允许（开发模式下 loopback IP 被豁免）
        assert!(v("https://127.0.0.1:8443/v1").is_ok());
    }

    // ---------- DNS 混合结果：同域名解析出公网+私网时整体拒绝 ----------

    #[test]
    fn rejects_domain_with_mixed_public_and_private_dns() {
        let r = validate_endpoint_with(
            "https://dual.example.com/v1",
            EndpointPolicy::DEFAULT,
            stub_mixed_resolver,
        );
        assert!(
            matches!(r, Err(EndpointError::Restricted(_))),
            "混合解析应整体拒绝: {:?}",
            r.err()
        );
    }

    // ---------- IP 受限判定（可注入式，无 DNS 依赖） ----------

    #[test]
    fn check_ip_matrix() {
        let deny_default = |s: &str| {
            assert!(
                check_ip_restrictions(IpAddr::from_str(s).unwrap(), EndpointPolicy::DEFAULT)
                    .is_err(),
                "{} 应被拒绝",
                s
            );
        };
        deny_default("127.0.0.1");
        deny_default("10.1.2.3");
        deny_default("172.31.255.255");
        deny_default("192.168.0.1");
        deny_default("169.254.169.254");
        deny_default("::1");
        deny_default("fe80::1");
        deny_default("::ffff:10.0.0.1"); // IPv4-mapped 私网
        deny_default("2002:0a00:0001::1"); // 6to4 封装 10.0.0.1
                                           // 公网地址放行
        assert!(check_ip_restrictions(
            IpAddr::from_str("8.8.8.8").unwrap(),
            EndpointPolicy::DEFAULT
        )
        .is_ok());
        assert!(check_ip_restrictions(
            IpAddr::from_str("2606:4700:4700::1111").unwrap(),
            EndpointPolicy::DEFAULT
        )
        .is_ok());
    }

    // ---------- 候选端点拼接 ----------

    #[test]
    fn model_candidates_join_rules() {
        let eps: Vec<String> = ok("https://api.example.com/v1")
            .model_candidates()
            .into_iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(eps, vec!["https://api.example.com/v1/models"]);

        let eps: Vec<String> = ok("https://api.example.com")
            .model_candidates()
            .into_iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            eps,
            vec![
                "https://api.example.com/v1/models",
                "https://api.example.com/models"
            ]
        );

        let eps: Vec<String> = ok("https://api.example.com/v1/models")
            .model_candidates()
            .into_iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(eps, vec!["https://api.example.com/v1/models"]);

        // 尾部斜杠与查询参数被归一，不产生双斜杠
        let eps: Vec<String> = ok("https://api.example.com/?x=1")
            .model_candidates()
            .into_iter()
            .map(|u| u.to_string())
            .collect();
        assert_eq!(
            eps,
            vec![
                "https://api.example.com/v1/models",
                "https://api.example.com/models"
            ]
        );
        assert!(!eps
            .iter()
            .any(|u| u.contains("//v1") || u.contains("api.example.com//")));
    }

    // ---------- 错误正文截断 ----------

    #[test]
    fn safe_error_body_truncates_at_char_boundary() {
        let big: String = "错".repeat(ERROR_BODY_LIMIT); // 每字 3 字节，远超上限
        let out = safe_error_body(&big);
        assert!(out.len() <= ERROR_BODY_LIMIT + "…(已截断)".len());
        assert!(out.ends_with("…(已截断)"));
        let small = "短正文";
        assert_eq!(safe_error_body(small), small);
    }

    // ---------- 安全日志表示 ----------

    #[test]
    fn safe_display_excludes_userinfo_and_query() {
        let ep = validate_endpoint_with(
            "https://api.example.com:8443/v1",
            EndpointPolicy::DEFAULT,
            stub_public_resolver,
        )
        .unwrap();
        let s = ep.safe_display();
        assert_eq!(s, "https://api.example.com:8443/v1");
    }
}
