use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use image::{imageops::FilterType, DynamicImage, ImageFormat, GenericImageView};
use moka::future::Cache;
use once_cell::sync::Lazy;
use reqwest::Client;
use sha1::{Digest, Sha1};
use std::{io::Cursor, net::SocketAddr, sync::Arc, time::Duration};
use tokio::signal;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    http: Client,
    cache: Cache<String, Arc<Vec<u8>>>, // final WebP bytes (static or animated)
}

static USER_AGENT: Lazy<String> = Lazy::new(|| {
    format!(
        "emoji-resizer/0.1 (+https://example.local) reqwest/0.12",
    )
});

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("emoji-resizer starting...");
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let http = Client::builder()
        .user_agent(USER_AGENT.clone())
        .http2_prior_knowledge()
        .pool_max_idle_per_host(32)
        .pool_idle_timeout(Duration::from_secs(30))
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .build()?;

    let cache = Cache::builder()
        .max_capacity(50_000) // 약 50k 개 항목(사이즈에 맞게 조절)
        .time_to_live(Duration::from_secs(24 * 3600))
        .build();

    let state = AppState { http, cache };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        // 예: GET /e/123456789012345678.webp
        .route("/e/:name", get(resize_handler))
        .with_state(state)
        .into_make_service_with_connect_info::<SocketAddr>(); 

    let addr: SocketAddr = "0.0.0.0:53292".parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("listening on http://{addr}");
    
    // Graceful shutdown 설정
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    
    info!("server shutdown complete");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            warn!("received Ctrl+C, shutting down gracefully");
        },
        _ = terminate => {
            warn!("received SIGTERM, shutting down gracefully");
        },
    }
}

async fn resize_handler(
    State(state): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    info!("Request received - Emoji ID: {}", name);

    // 확장자 제거 (.webp, .gif, .png 등)
    let emoji_id = name
        .split('.')
        .next()
        .unwrap_or(&name)
        .to_string();
    
    info!("Processed Emoji ID: {}", emoji_id);

    // 캐시 키: 이모지 ID만 사용 (고정 크기 160x160, WebP 포맷)
    let key = emoji_id.clone();

    if let Some(bytes) = state.cache.get(&key).await {
        info!("Cache hit for emoji: {}", emoji_id);
        let etag = make_etag(&bytes);
        if header_matches(&headers, header::IF_NONE_MATCH, &etag) {
            return (StatusCode::NOT_MODIFIED, with_common_headers(etag, None)).into_response();
        }
        return (
            with_common_headers(etag, Some(&format_src(&emoji_id))),
            bytes.as_ref().clone(),
        )
            .into_response();
    }

    info!("Cache miss - fetching emoji: {}", emoji_id);

    // 원본 URL 구성: Discord CDN (애니메이션 WebP 지원)
    let src = format!(
        "https://cdn.discordapp.com/emojis/{}?size=160&animated=true",
        emoji_id
    );

    // 원본 fetch
    let resp = match state
        .http
        .get(&src)
        .header(header::ACCEPT, "image/webp,image/*")
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            error!("Fetch error for emoji {}: {}", emoji_id, e);
            return (StatusCode::BAD_GATEWAY, "upstream fetch failed").into_response();
        }
    };

    if resp.status() == StatusCode::NOT_FOUND {
        warn!("Emoji not found: {}", emoji_id);
        return (StatusCode::NOT_FOUND, "emoji not found").into_response();
    }
    if !resp.status().is_success() {
        error!("Upstream error for emoji {}: status {}", emoji_id, resp.status());
        return (StatusCode::BAD_GATEWAY, "upstream error").into_response();
    }

    let body = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            error!("Read body error for emoji {}: {}", emoji_id, e);
            return (StatusCode::BAD_GATEWAY, "upstream read failed").into_response();
        }
    };

    // WebP 파일 헤더 분석으로 애니메이션 여부 확인
    let is_animated = is_animated_webp(&body);
    
    if is_animated {
        info!("Processing animated WebP emoji: {}", emoji_id);
        // 애니메이션 WebP는 그대로 반환 (현재 image crate는 애니메이션 WebP 리사이징 미지원)
        let bytes = Arc::new(body.to_vec());
        state.cache.insert(key, bytes.clone()).await;
        
        let etag = make_etag(&bytes);
        info!("Animated WebP processed - emoji: {}, size: {} bytes", 
              emoji_id, bytes.len());
        
        return (
            with_common_headers(etag, Some(&format_src(&emoji_id))),
            bytes.as_ref().clone(),
        )
            .into_response();
    }

    // 정적 WebP 처리: 디코드 → 종횡비 유지하며 리사이즈 → WebP 인코드
    let img: DynamicImage = match image::load_from_memory(&body) {
        Ok(i) => i,
        Err(e) => {
            error!("Decode error for emoji {}: {}", emoji_id, e);
            return (StatusCode::UNSUPPORTED_MEDIA_TYPE, "decode failed").into_response();
        }
    };

    let original_dimensions = img.dimensions();
    info!("Static WebP processing - emoji: {}, original: {}x{}", 
          emoji_id, original_dimensions.0, original_dimensions.1);

    // 종횡비를 유지하면서 160x160 박스 안에 맞는 최대 크기로 리사이즈
    let resized = img.resize(160, 160, FilterType::Lanczos3);
    let final_dimensions = resized.dimensions();
    
    let mut out = Vec::new();
    if let Err(e) = resized.write_to(&mut Cursor::new(&mut out), ImageFormat::WebP) {
        error!("Encode error for emoji {}: {}", emoji_id, e);
        return (StatusCode::INTERNAL_SERVER_ERROR, "encode failed").into_response();
    }
    let bytes = Arc::new(out);

    // 캐시 저장
    state.cache.insert(key, bytes.clone()).await;

    info!("Static WebP processed - emoji: {}, {}x{} → {}x{}, size: {} bytes", 
          emoji_id, original_dimensions.0, original_dimensions.1, 
          final_dimensions.0, final_dimensions.1, bytes.len());

    let etag = make_etag(&bytes);
    (
        with_common_headers(etag, Some(&format_src(&emoji_id))),
        bytes.as_ref().clone(),
    )
        .into_response()
}

fn is_animated_webp(data: &[u8]) -> bool {
    // WebP 파일 시그니처 확인: "RIFF????WEBP"
    if data.len() < 12 {
        return false;
    }
    
    if &data[0..4] != b"RIFF" || &data[8..12] != b"WEBP" {
        return false;
    }
    
    // VP8X 청크가 있는지 확인 (확장 기능을 나타냄)
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let chunk_type = &data[pos..pos + 4];
        let chunk_size = u32::from_le_bytes([
            data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]
        ]) as usize;
        
        if chunk_type == b"VP8X" {
            // VP8X 플래그에서 애니메이션 비트(bit 1) 확인
            if pos + 8 < data.len() {
                let flags = data[pos + 8];
                return (flags & 0x02) != 0; // 애니메이션 플래그
            }
            return false;
        }
        
        // ANIM 청크가 있으면 애니메이션
        if chunk_type == b"ANIM" {
            return true;
        }
        
        pos += 8 + chunk_size;
        // 홀수 크기인 경우 패딩 바이트 추가
        if chunk_size % 2 == 1 {
            pos += 1;
        }
    }
    
    false
}

fn make_etag(bytes: &[u8]) -> String {
    let hash = Sha1::digest(bytes);
    format!("W/\"{:x}\"", hash)
}

fn header_matches(headers: &HeaderMap, name: header::HeaderName, value: &str) -> bool {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').any(|t| t.trim() == value))
        .unwrap_or(false)
}

fn with_common_headers(
    etag: String,
    src: Option<&str>,
) -> [(header::HeaderName, String); 4] {
    [
        (header::CONTENT_TYPE, "image/webp".into()),
        (
            header::CACHE_CONTROL,
            "public, max-age=86400, stale-while-revalidate=600".into(),
        ),
        (header::ETAG, etag),
        (header::HeaderName::from_static("x-source-url"), src.unwrap_or("-").into()),
    ]
}

fn format_src(name: &str) -> String {
    format!(
        "https://cdn.discordapp.com/emojis/{}?size=160&animated=true",
        name
    )
}
