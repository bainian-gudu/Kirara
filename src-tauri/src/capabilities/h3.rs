//! H3（HTTP/3 over QUIC）客户端中间件。
//!
//! 传输层是 `quinn` + `rustls`，替换掉上游的 `h3-msquic-async` +
//! `xytoki/msquic-async-rs` fork + 静态 `seera-msquic`
//! （理由、影响与回退方式见 ../../LOCAL_PATCHES.md 第 15 节）。
//!
//! 对外行为保持不变：
//! - 连接按 `(host, port, 固定配置)` 复用，空闲 / 已死连接会被清扫，池子上限 32
//! - 证书固定语义不变，见 [`PinningMode`] / [`PinTarget`]
//! - URL 片段里没有固定值时只做系统证书验证
//! - [`H3Middleware::discover`] 接受任意证书并回传算出来的哈希

use async_trait::async_trait;
use bytes::{Buf, Bytes};
use futures::future::poll_fn;
use reqwest_middleware::{Middleware, Next};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, trace, warn};

// ============================================================
// 固定模式与配置
// ============================================================

/// 控制证书固定校验与系统证书验证之间的关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinningMode {
    /// 无论系统是否信任证书，始终检查固定值（默认）。
    /// 即使系统验证通过，固定值也必须匹配。
    Force,
    /// 仅在系统不信任证书时检查固定值。
    /// 系统信任证书时直接接受，不检查固定值。
    Add,
}

/// 固定校验的目标：SPKI 哈希或完整证书哈希。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinTarget {
    /// SubjectPublicKeyInfo 的 DER 编码的 SHA-256 哈希。
    Spki([u8; 32]),
    /// 完整证书 DER 编码的 SHA-256 哈希。
    /// 对应命令：`openssl x509 -in cert.crt -outform DER | openssl dgst -sha256 -binary | xxd -p -c 32`
    Cert([u8; 32]),
}

/// 从 URL 片段解析得到的证书固定配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PinConfig {
    pub target: PinTarget,
    pub mode: PinningMode,
}

// ============================================================
// 证书哈希：极简 DER 定位 + SHA-256
// ============================================================

/// SHA-256。
fn sha256(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// 从 X.509 证书的 DER 里取出 SubjectPublicKeyInfo 的**整段** DER（含 tag 与长度）。
///
/// 上游走 Windows CryptoAPI：`CryptEncodeObjectEx(X509_PUBLIC_KEY_INFO)` 把 Schannel
/// 解析出来的公钥重新编码一遍。这里改为在原始 DER 上直接定位同一段 —— 对同一张证书
/// 两者字节相同，所以 `openssl x509 -pubkey -noout | openssl pkey -pubin -outform DER
/// | sha256sum` 的结果仍然可以直接当固定值用。
///
/// 只走 `Certificate → TBSCertificate → 第 7 个字段` 这条路径，不做任何猜测：
/// 长度越界、不定长编码（DER 不允许）、tag 不对，一律返回 None。
fn extract_spki_der(cert_der: &[u8]) -> Option<&[u8]> {
    /// 读一个 TLV，返回 (tag, value, 整段 TLV)，并把游标推到下一个 TLV。
    fn read_tlv<'a>(buf: &'a [u8], pos: &mut usize) -> Option<(u8, &'a [u8], &'a [u8])> {
        let start = *pos;
        let tag = *buf.get(start)?;
        let first_len = *buf.get(start + 1)?;
        let mut cursor = start + 2;

        let len = if first_len & 0x80 == 0 {
            first_len as usize
        } else {
            let n = (first_len & 0x7f) as usize;
            // 0x80 是不定长（DER 不允许）；证书长度不会超过 4 字节。
            if n == 0 || n > 4 {
                return None;
            }
            let mut value = 0usize;
            for _ in 0..n {
                value = (value << 8) | *buf.get(cursor)? as usize;
                cursor += 1;
            }
            value
        };

        let end = cursor.checked_add(len)?;
        let full = buf.get(start..end)?;
        *pos = end;
        Some((tag, buf.get(cursor..end)?, full))
    }

    let mut pos = 0;
    let (cert_tag, cert_body, _) = read_tlv(cert_der, &mut pos)?;
    if cert_tag != 0x30 {
        return None;
    }

    let mut pos = 0;
    let (tbs_tag, tbs_body, _) = read_tlv(cert_body, &mut pos)?;
    if tbs_tag != 0x30 {
        return None;
    }

    // TBSCertificate ::= SEQUENCE {
    //     version [0] EXPLICIT Version DEFAULT v1,   -- 可选
    //     serialNumber, signature, issuer, validity, subject,
    //     subjectPublicKeyInfo, ... }
    let mut pos = 0;
    let (first_tag, _first_value, _first_full) = read_tlv(tbs_body, &mut pos)?;
    // 读到 version 就还剩 5 个字段，否则刚读到的就是 serialNumber。
    let remaining = if first_tag == 0xa0 { 5 } else { 4 };
    for _ in 0..remaining {
        read_tlv(tbs_body, &mut pos)?;
    }

    let (spki_tag, _spki_value, spki_full) = read_tlv(tbs_body, &mut pos)?;
    if spki_tag != 0x30 {
        return None;
    }
    Some(spki_full)
}

/// 计算证书 SPKI 的 SHA-256；DER 结构不认识时返回 None。
fn compute_spki_hash(cert_der: &[u8]) -> Option<[u8; 32]> {
    extract_spki_der(cert_der).map(sha256)
}

/// 计算整张证书 DER 的 SHA-256。
fn compute_cert_hash(cert_der: &[u8]) -> [u8; 32] {
    sha256(cert_der)
}

// ============================================================
// 证书验证器：系统验证（+ 可选的固定值校验）
// ============================================================

/// 从服务器证书中发现的哈希值。
#[derive(Debug, Clone, Default)]
pub struct DiscoveredHashes {
    pub spki: Option<[u8; 32]>,
    pub cert: Option<[u8; 32]>,
}

/// 证书验证的四种形态。
enum VerifyMode {
    /// 只用系统证书验证器（URL 片段里没给固定值）。
    SystemOnly,
    /// 系统验证 + 固定值校验，关系由 [`PinningMode`] 决定。
    Pin(PinConfig),
    /// 发现模式：接受任何证书，把算出来的哈希写进这里（`discover()` 用）。
    Discovery(Arc<Mutex<DiscoveredHashes>>),
    /// 构造基础配置时的占位：什么证书都不接受。
    /// 真实连接都会把验证器换成上面三种之一，这个形态不会走到握手。
    RejectAll,
}

/// 包一层系统证书验证器（Windows 上是 CryptoAPI 证书链验证，等价于上游的 Schannel），
/// 再按 [`VerifyMode`] 决定要不要额外比对固定值。
struct PinVerifier {
    inner: Arc<rustls_platform_verifier::Verifier>,
    mode: VerifyMode,
}

impl std::fmt::Debug for PinVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinVerifier")
            .field("mode", &self.mode_name())
            .finish()
    }
}

impl PinVerifier {
    fn new(inner: Arc<rustls_platform_verifier::Verifier>, mode: VerifyMode) -> Self {
        Self { inner, mode }
    }

    fn mode_name(&self) -> &'static str {
        match self.mode {
            VerifyMode::SystemOnly => "system",
            VerifyMode::Pin(_) => "pin",
            VerifyMode::Discovery(_) => "discovery",
            VerifyMode::RejectAll => "reject-all",
        }
    }

    /// 系统是否信任这张证书（等价于上游的 `deferred_status.is_ok()`）。
    fn system_trusts(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> bool {
        self.inner
            .verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
            .is_ok()
    }

    /// 比对固定值：命中返回断言，不命中返回证书错误。
    fn check_pin(
        &self,
        config: &PinConfig,
        cert_der: &[u8],
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &config.target {
            PinTarget::Spki(expected) => match compute_spki_hash(cert_der) {
                Some(got) if got == *expected => {
                    debug!("[Pin] SPKI pin MATCHED");
                    Ok(ServerCertVerified::assertion())
                }
                Some(got) => {
                    warn!(
                        "[Pin] SPKI pin MISMATCH! expected={}, got={}",
                        hex::encode(expected),
                        hex::encode(got)
                    );
                    Err(pin_error("SPKI pin mismatch"))
                }
                None => {
                    warn!("[Pin] Failed to parse SubjectPublicKeyInfo, rejecting");
                    Err(pin_error("malformed certificate"))
                }
            },
            PinTarget::Cert(expected) => {
                let got = compute_cert_hash(cert_der);
                if got == *expected {
                    debug!("[Pin] Cert pin MATCHED");
                    Ok(ServerCertVerified::assertion())
                } else {
                    warn!(
                        "[Pin] Cert pin MISMATCH! expected={}, got={}",
                        hex::encode(expected),
                        hex::encode(got)
                    );
                    Err(pin_error("certificate pin mismatch"))
                }
            }
        }
    }
}

/// 构造一个「证书不被接受」的 rustls 错误（与系统验证器的错误同一类型）。
fn pin_error(message: &str) -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::Other(rustls::OtherError(
        Arc::from(Box::<dyn std::error::Error + Send + Sync>::from(
            message.to_string(),
        )),
    )))
}

impl ServerCertVerifier for PinVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.mode {
            VerifyMode::SystemOnly => self.inner.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
            VerifyMode::Discovery(results) => {
                let cert_der = end_entity.as_ref();
                let spki = compute_spki_hash(cert_der);
                let cert = compute_cert_hash(cert_der);
                let system_trusts =
                    self.system_trusts(end_entity, intermediates, server_name, ocsp_response, now);

                match spki {
                    Some(hash) => tracing::info!("[Discovery] SPKI SHA-256: {}", hex::encode(hash)),
                    None => warn!("[Discovery] Failed to parse SubjectPublicKeyInfo"),
                }
                tracing::info!(
                    "[Discovery] system_trusts={}, Cert SHA-256: {}",
                    system_trusts,
                    hex::encode(cert)
                );

                if let Ok(mut guard) = results.lock() {
                    guard.spki = spki;
                    guard.cert = Some(cert);
                }

                // 发现模式的目的就是拿到哈希：无论系统是否信任都接受。
                Ok(ServerCertVerified::assertion())
            }
            VerifyMode::Pin(config) => {
                let cert_der = end_entity.as_ref();
                let system_trusts =
                    self.system_trusts(end_entity, intermediates, server_name, ocsp_response, now);

                match config.mode {
                    PinningMode::Add if system_trusts => {
                        debug!("[Pin] mode=add, system trusts, accepting");
                        Ok(ServerCertVerified::assertion())
                    }
                    PinningMode::Add => {
                        debug!("[Pin] mode=add, system rejects, checking pin...");
                        self.check_pin(config, cert_der)
                    }
                    PinningMode::Force => {
                        debug!(
                            "[Pin] mode=force, system_trusts={}, checking pin...",
                            system_trusts
                        );
                        self.check_pin(config, cert_der)
                    }
                }
            }
            VerifyMode::RejectAll => Err(pin_error("no certificate verifier for this connection")),
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
}

// ============================================================
// URL 片段解析器：#spki={hex}&cert={hex}&pinning_mode=force|add
// ============================================================

/// 从 URL 片段解析证书固定配置。
///
/// 支持的格式：
///   `#spki={hex64}`                           → PinConfig { target: Spki, mode: Force }
///   `#spki={hex64}&pinning_mode=add`          → PinConfig { target: Spki, mode: Add }
///   `#cert={hex64}`                           → PinConfig { target: Cert, mode: Force }
///   `#cert={hex64}&pinning_mode=force`        → PinConfig { target: Cert, mode: Force }
///   `#spki={hex64}&cert={hex64}&pinning_mode=force` → PinConfig { target: Cert, mode: Force }
///                                               (cert takes priority over spki)
fn parse_pin_from_fragment(url: &url::Url) -> Option<PinConfig> {
    let frag = url.fragment()?;

    let mut spki_hex: Option<&str> = None;
    let mut cert_hex: Option<&str> = None;
    let mut mode = PinningMode::Force; // 默认值

    for part in frag.split('&') {
        if let Some(val) = part.strip_prefix("spki=") {
            spki_hex = Some(val);
        } else if let Some(val) = part.strip_prefix("cert=") {
            cert_hex = Some(val);
        } else if let Some(val) = part.strip_prefix("pinning_mode=") {
            mode = match val {
                "add" => PinningMode::Add,
                _ => PinningMode::Force, // 未知值 → force（安全默认值）
            };
        }
    }

    // 证书优先于 SPKI
    let (hex_str, make_target): (&str, fn([u8; 32]) -> PinTarget) = if let Some(h) = cert_hex {
        (h, PinTarget::Cert)
    } else if let Some(h) = spki_hex {
        (h, PinTarget::Spki)
    } else {
        return None;
    };

    // 保护: SHA-256 hex 必须 为 exactly 64 chars, reject 尽早 到 avoid large alloc
    if hex_str.len() != 64 {
        return None;
    }
    let bytes = hex::decode(hex_str).ok()?;
    let hash = <[u8; 32]>::try_from(bytes.as_slice()).ok()?;
    Some(PinConfig {
        target: make_target(hash),
        mode,
    })
}

// ============================================================
// 连接池
// ============================================================

/// 活动流守卫，避免连接被当作空闲连接移除。
struct ActiveStreamGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ActiveStreamGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

pub type H3SendRequest = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;

/// 连接池中的最大连接数。
const MAX_POOL_SIZE: usize = 32;

struct H3ConnEntry {
    send_request: H3SendRequest,
    cancel_token: CancellationToken,
    driver_handle: JoinHandle<()>,
    close_rx: std::sync::mpsc::Receiver<()>,
    last_used: Instant,
    active_streams: Arc<AtomicUsize>,
}

impl Drop for H3ConnEntry {
    fn drop(&mut self) {
        debug!("[H3ConnEntry] Cancelling and aborting H3 driver task");
        self.cancel_token.cancel();
        self.driver_handle.abort(); // P1-2：同时中止驱动任务
    }
}

/// 按地址族各留一个 UDP 端点。
///
/// 不共用一个双栈 socket：`IPV6_V6ONLY` 的默认值各平台不一致，分开之后
/// 「IPv4 目标用 IPv4 socket、IPv6 目标用 IPv6 socket」是确定的。
#[derive(Default)]
struct EndpointPool {
    v4: Option<quinn::Endpoint>,
    v6: Option<quinn::Endpoint>,
}

impl EndpointPool {
    fn get(&mut self, ipv6: bool) -> anyhow::Result<quinn::Endpoint> {
        let slot = if ipv6 { &mut self.v6 } else { &mut self.v4 };
        if let Some(endpoint) = slot {
            return Ok(endpoint.clone());
        }

        let bind: std::net::SocketAddr = if ipv6 {
            "[::]:0".parse().expect("字面量地址")
        } else {
            "0.0.0.0:0".parse().expect("字面量地址")
        };
        let endpoint = quinn::Endpoint::client(bind)?;
        *slot = Some(endpoint.clone());
        Ok(endpoint)
    }
}

type PoolKey = (String, u16, Option<PinConfig>);

/// 规范化主机名，确保连接池键匹配一致。
/// - 转为小写（DNS 不区分大小写）
/// - 移除 IPv6 方括号（url::Url 会自动添加）
fn normalize_host(host: &str) -> String {
    let h = host.to_ascii_lowercase();
    if h.starts_with('[') && h.ends_with(']') {
        h[1..h.len() - 1].to_string()
    } else {
        h
    }
}

// ============================================================
// 修复 2：H3Inner 通过 Arc 共享状态，在响应体存活期间保持连接
// ============================================================

struct H3Inner {
    /// 系统证书验证器：Windows 上就是 CryptoAPI 证书链验证。
    platform_verifier: Arc<rustls_platform_verifier::Verifier>,
    /// 只带 ALPN=h3 的 TLS 1.3 配置模板；每个连接克隆一份再换掉验证器。
    base_tls_config: rustls::ClientConfig,
    /// QUIC 传输参数（空闲超时、并发流上限）。
    transport: Arc<quinn::TransportConfig>,
    /// 每个地址族一个 UDP 端点，懒创建。
    endpoints: tokio::sync::Mutex<EndpointPool>,
    pool: Mutex<HashMap<PoolKey, H3ConnEntry>>,
    idle_timeout: Duration,
}

impl H3Inner {
    fn lock_pool(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<PoolKey, H3ConnEntry>>, reqwest_middleware::Error>
    {
        self.pool.lock().map_err(|_| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("H3 pool mutex poisoned"))
        })
    }
}

impl Drop for H3Inner {
    fn drop(&mut self) {
        debug!("[H3Inner] Dropping, cleaning up resources");
        if let Ok(mut pool) = self.pool.lock() {
            pool.drain();
        }
        debug!("[H3Inner] Dropped successfully");
    }
}

// ============================================================
// 相关实现：H3 Middleware
// ============================================================

pub struct H3Middleware {
    inner: Arc<H3Inner>,
}

/// 启动探测：构建一次完整的 QUIC 客户端配置。
///
/// 等价于上游「能不能建出 msquic Registration + Schannel 凭据」的那次探测：
/// 这里验证 ring 加密提供者、系统证书验证器和 QUIC 参数是否都能就绪。
/// 只建配置、不开 socket，代价可以忽略。
pub fn probe() -> anyhow::Result<()> {
    H3Middleware::new(Duration::from_secs(60)).map(|_| ())
}

impl H3Middleware {
    pub fn new(idle_timeout: Duration) -> anyhow::Result<Self> {
        // 显式指定 ring：本仓库的 rustls 同时开着 aws-lc-rs（russh 要的），
        // 走 `ClientConfig::builder()` 会因为「默认提供者不唯一」直接 panic。
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let platform_verifier = Arc::new(rustls_platform_verifier::Verifier::new(Arc::clone(
            &provider,
        ))?);

        let mut base_tls_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinVerifier::new(
                Arc::clone(&platform_verifier),
                VerifyMode::RejectAll,
            )))
            .with_no_client_auth();
        base_tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let mut transport = quinn::TransportConfig::default();
        transport
            .max_idle_timeout(Some(quinn::IdleTimeout::try_from(idle_timeout)?))
            .max_concurrent_bidi_streams(100u32.into())
            .max_concurrent_uni_streams(100u32.into());

        Ok(Self {
            inner: Arc::new(H3Inner {
                platform_verifier,
                base_tls_config,
                transport: Arc::new(transport),
                endpoints: tokio::sync::Mutex::new(EndpointPool::default()),
                pool: Mutex::new(HashMap::new()),
                idle_timeout,
            }),
        })
    }

    /// 组装一个 quinn 客户端配置：克隆 TLS 模板、换掉证书验证器、挂上传输参数。
    fn client_config(&self, mode: VerifyMode) -> anyhow::Result<quinn::ClientConfig> {
        let mut tls_config = self.inner.base_tls_config.clone();
        tls_config
            .dangerous()
            .set_certificate_verifier(Arc::new(PinVerifier::new(
                Arc::clone(&self.inner.platform_verifier),
                mode,
            )));

        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls_config)?;
        let mut config = quinn::ClientConfig::new(Arc::new(crypto));
        config.transport_config(Arc::clone(&self.inner.transport));
        Ok(config)
    }

    /// 建一条 QUIC 连接（DNS 解析 + 按地址族取端点 + 握手）。
    async fn connect(
        &self,
        host: &str,
        port: u16,
        mode: VerifyMode,
    ) -> Result<quinn::Connection, reqwest_middleware::Error> {
        let resolved = tokio::net::lookup_host((host, port)).await.map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] DNS lookup failed");
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 dns: {e}"))
        })?;
        let addresses: Vec<std::net::SocketAddr> = resolved.collect();
        // 优先 IPv4：绝大多数下载源都同时有 A / AAAA 记录，先用 A 记录能少一次
        // 「IPv6 不通再退回来」的等待。
        let addr = addresses
            .iter()
            .find(|addr| addr.is_ipv4())
            .or_else(|| addresses.first())
            .copied()
            .ok_or_else(|| {
                warn!(host = %host, port, "[H3] DNS returned no address");
                reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 dns: no address"))
            })?;

        let endpoint = {
            let mut endpoints = self.inner.endpoints.lock().await;
            endpoints.get(addr.is_ipv6()).map_err(|e| {
                warn!(error = %e, "[H3] Failed to open UDP endpoint");
                reqwest_middleware::Error::Middleware(e)
            })?
        };

        let config = self.client_config(mode).map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] Client config failed");
            reqwest_middleware::Error::Middleware(e)
        })?;

        let connecting = endpoint.connect_with(config, addr, host).map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] connect_with failed");
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 connect: {e}"))
        })?;

        connecting.await.map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] QUIC handshake failed");
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 handshake: {e}"))
        })
    }

    async fn get_send_request(
        &self,
        host: &str,
        port: u16,
        pin_config: Option<PinConfig>,
    ) -> Result<(H3SendRequest, ActiveStreamGuard), reqwest_middleware::Error> {
        let norm_host = normalize_host(host);
        let key = (norm_host.clone(), port, pin_config);

        // 检查 连接池 (hold std::sync::Mutex briefly, 发布 之前 .等待)
        {
            let mut pool = self.inner.lock_pool()?;

            // P1-1: Sweep 过期 connections 和 enforce max 连接池 大小
            let now = Instant::now();
            let keys_to_evict: Vec<PoolKey> = pool
                .iter()
                .filter_map(|(k, entry)| {
                    let dead = matches!(
                        entry.close_rx.try_recv(),
                        Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected)
                    );
                    let driver_done = entry.driver_handle.is_finished();
                    let idle_expired =
                        now.duration_since(entry.last_used) > self.inner.idle_timeout;
                    let active = entry.active_streams.load(Ordering::Relaxed);
                    if dead || driver_done || (idle_expired && active == 0) {
                        Some(k.clone())
                    } else {
                        None
                    }
                })
                .collect();
            for k in keys_to_evict {
                debug!(host = %k.0, port = k.1, "[H3] Sweep: evicting stale connection");
                pool.remove(&k);
            }

            // 移除最旧条目，限制连接池的最大大小
            if pool.len() >= MAX_POOL_SIZE {
                let mut entries: Vec<_> =
                    pool.iter().map(|(k, e)| (k.clone(), e.last_used)).collect();
                entries.sort_by_key(|(_, last_used)| *last_used);
                let to_remove = pool.len() - MAX_POOL_SIZE + 1;
                let keys_to_evict: Vec<PoolKey> = entries
                    .into_iter()
                    .take(to_remove)
                    .map(|(k, _)| k)
                    .collect();
                for k in keys_to_evict {
                    debug!(host = %k.0, port = k.1, "[H3] Pool full: evicting oldest");
                    pool.remove(&k);
                }
            }

            if let Some(entry) = pool.get_mut(&key) {
                let dead = match entry.close_rx.try_recv() {
                    Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => true,
                    Err(std::sync::mpsc::TryRecvError::Empty) => false,
                };
                let idle_expired =
                    Instant::now().duration_since(entry.last_used) > self.inner.idle_timeout;
                let driver_done = entry.driver_handle.is_finished();
                let active = entry.active_streams.load(Ordering::Relaxed);

                if dead || driver_done || (idle_expired && active == 0) {
                    let reason = if dead {
                        "dead"
                    } else if driver_done {
                        "driver_finished"
                    } else {
                        "idle"
                    };
                    debug!(host = %norm_host, port, reason, "[H3] Evicting stale connection");
                    pool.remove(&key);
                } else {
                    debug!(host = %norm_host, port, "[H3] Reusing existing connection");
                    entry.last_used = Instant::now();
                    entry.active_streams.fetch_add(1, Ordering::Relaxed);
                    let guard = ActiveStreamGuard {
                        counter: Arc::clone(&entry.active_streams),
                    };
                    return Ok((entry.send_request.clone(), guard));
                }
            }
        }
        // 已释放锁

        let mode = match pin_config {
            Some(config) => VerifyMode::Pin(config),
            None => VerifyMode::SystemOnly,
        };

        debug!(host = %norm_host, port, pin = ?pin_config, "[H3] Creating new QUIC connection");
        let conn = self.connect(&norm_host, port, mode).await?;

        debug!(host = %norm_host, port, "[H3] QUIC connection established");

        let h3_conn = h3_quinn::Connection::new(conn);
        let (mut driver, send_request) = h3::client::new(h3_conn).await.map_err(|e| {
            warn!(host = %norm_host, port, error = %e, "[H3] h3 handshake failed");
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 handshake: {}", e))
        })?;

        let cancel_token = CancellationToken::new();
        let child_token = cancel_token.child_token();
        let (close_tx, close_rx) = std::sync::mpsc::channel();

        let driver_handle = tokio::spawn(async move {
            tokio::select! {
                _ = poll_fn(|cx| driver.poll_close(cx)) => {
                    close_tx.send(()).ok();
                    trace!("[driver] poll_close completed");
                }
                _ = child_token.cancelled() => {
                    trace!("[driver] Cancelled");
                }
            }
        });

        let new_entry = H3ConnEntry {
            send_request,
            cancel_token,
            driver_handle,
            close_rx,
            last_used: Instant::now(),
            active_streams: Arc::new(AtomicUsize::new(0)),
        };

        // 以竞态安全方式插入连接池
        {
            let mut pool = self.inner.lock_pool()?;
            if let Some(existing) = pool.get_mut(&key) {
                debug!(host = %norm_host, port, "[H3] Race: using existing connection");
                existing.last_used = Instant::now();
                existing.active_streams.fetch_add(1, Ordering::Relaxed);
                let guard = ActiveStreamGuard {
                    counter: Arc::clone(&existing.active_streams),
                };
                return Ok((existing.send_request.clone(), guard));
            }
            new_entry.active_streams.fetch_add(1, Ordering::Relaxed);
            let guard = ActiveStreamGuard {
                counter: Arc::clone(&new_entry.active_streams),
            };
            let cloned = new_entry.send_request.clone();
            pool.insert(key, new_entry);
            Ok((cloned, guard))
        }
    }

    /// 执行 H3/QUIC 请求。
    ///
    /// 注意：仅支持不带请求体的 GET 请求；传入的请求体会被忽略。
    /// 用于服务器提供内容的文件下载场景。
    pub async fn h3_request(
        &self,
        req: reqwest::Request,
    ) -> Result<reqwest::Response, reqwest_middleware::Error> {
        let method = req.method().clone();
        let headers = req.headers().clone();
        let original_url = req.url().clone();

        let host = original_url
            .host_str()
            .ok_or_else(|| {
                reqwest_middleware::Error::Middleware(anyhow::anyhow!("no host in URL"))
            })?
            .to_string();
        let port = original_url.port().unwrap_or(443);
        let path_and_query = match original_url.query() {
            Some(q) => format!("{}?{}", original_url.path(), q),
            None => original_url.path().to_string(),
        };
        // URI 的主机部分需要用方括号包裹 IPv6 地址，例如 https://[::1]:443/path
        // url.host_str() 返回的 IPv6 地址可能已包含方括号，因此先检查
        let authority_host = if host.starts_with('[') {
            host.clone() // 已经 has brackets
        } else if host.contains(':') {
            format!("[{}]", host) // IPv6 地址尚未包含方括号
        } else {
            host.clone() // IPv4 地址或主机名
        };
        let h3_uri = format!("https://{}:{}{}", authority_host, port, path_and_query);

        // 从 URL 片段解析固定配置：#spki={hex}&cert={hex}&pinning_mode=force|add
        let pin_config = parse_pin_from_fragment(&original_url);

        debug!(url = %original_url, h3_uri = %h3_uri, pin = ?pin_config, "[H3] Intercepted");

        let (mut send_request, stream_guard) =
            self.get_send_request(&host, port, pin_config).await?;

        let mut h3_req_builder = http::Request::builder()
            .method(method.as_str())
            .uri(&h3_uri);
        for (name, value) in headers.iter() {
            h3_req_builder = h3_req_builder.header(name, value);
        }
        let h3_req = h3_req_builder
            .body(())
            .map_err(|e| reqwest_middleware::Error::Middleware(e.into()))?;

        // 复制键，供错误路径使用
        let pool_key = (normalize_host(&host), port, pin_config);

        let mut stream = send_request.send_request(h3_req).await.map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] send_request failed, evicting");
            if let Ok(mut pool) = self.inner.pool.lock() {
                pool.remove(&pool_key);
            }
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 send: {}", e))
        })?;

        // P1-4：完成或 recv_response 出错时也移除连接
        stream.finish().await.map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] finish failed, evicting");
            if let Ok(mut pool) = self.inner.pool.lock() {
                pool.remove(&pool_key);
            }
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 finish: {}", e))
        })?;

        let h3_resp = stream.recv_response().await.map_err(|e| {
            warn!(host = %host, port, error = %e, "[H3] recv_response failed, evicting");
            if let Ok(mut pool) = self.inner.pool.lock() {
                pool.remove(&pool_key);
            }
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 recv_response: {}", e))
        })?;

        debug!(status = %h3_resp.status(), "[H3] Got response");

        let (parts, _) = h3_resp.into_parts();
        let inner_arc = Arc::clone(&self.inner);

        let byte_stream = async_stream::try_stream! {
            let _keep_inner = inner_arc;
            let _keep_sr = send_request;
            let _stream_guard = stream_guard;

            loop {
                match stream.recv_data().await {
                    Ok(Some(mut data)) => {
                        yield data.copy_to_bytes(data.remaining());
                    }
                    Ok(None) => break,
                    Err(e) => {
                        Err(std::io::Error::other(
                            format!("h3 recv_data error: {e}"),
                        ))?;
                    }
                }
            }
        };

        let mut builder = http::Response::builder().status(parts.status);
        for (name, value) in &parts.headers {
            builder = builder.header(name, value);
        }
        let pinned: std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<Bytes, std::io::Error>> + Send>,
        > = Box::pin(byte_stream);
        let body = reqwest::Body::wrap_stream(pinned);
        let http_response = builder
            .body(body)
            .map_err(|e| reqwest_middleware::Error::Middleware(e.into()))?;

        let response: reqwest::Response = http_response.into();
        Ok(response)
    }

    #[allow(dead_code)]
    pub async fn shutdown(&self) {
        debug!("[H3] Shutting down...");
        let driver_handles: Vec<JoinHandle<()>> = {
            if let Ok(mut pool) = self.inner.pool.lock() {
                let mut handles = Vec::new();
                for (_, mut entry) in pool.drain() {
                    entry.cancel_token.cancel();
                    handles.push(std::mem::replace(
                        &mut entry.driver_handle,
                        tokio::spawn(async {}),
                    ));
                }
                handles
            } else {
                Vec::new()
            }
        };
        for handle in driver_handles {
            handle.abort();
        }
        debug!("[H3] Shut down complete.");
    }

    /// 发现远程服务器证书的 SPKI 和完整证书 SHA-256 哈希。
    /// 建立一次性 QUIC 连接，通过发现模式的验证器提取哈希后关闭连接。
    pub async fn discover(
        &self,
        host: &str,
        port: u16,
    ) -> Result<DiscoveredHashes, reqwest_middleware::Error> {
        let results = Arc::new(Mutex::new(DiscoveredHashes::default()));
        let norm_host = normalize_host(host);

        let conn = self
            .connect(
                &norm_host,
                port,
                VerifyMode::Discovery(Arc::clone(&results)),
            )
            .await?;

        // 执行最小 H3 握手，确保触发证书验证
        let h3_conn = h3_quinn::Connection::new(conn);
        let (_driver, _send_request) = h3::client::new(h3_conn).await.map_err(|e| {
            reqwest_middleware::Error::Middleware(anyhow::anyhow!("h3 discover: {}", e))
        })?;

        let result = results
            .lock()
            .map_err(|_| reqwest_middleware::Error::Middleware(anyhow::anyhow!("mutex poisoned")))?
            .clone();

        Ok(result)
    }
}

#[async_trait]
impl Middleware for H3Middleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut http::Extensions,
        next: Next<'_>,
    ) -> Result<reqwest::Response, reqwest_middleware::Error> {
        if req.url().scheme() == "http3" {
            self.h3_request(req).await
        } else {
            next.run(req, extensions).await
        }
    }
}
