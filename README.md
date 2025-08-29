# Emoji Resizer CDN

Discord 이모지를 100x100에서 160x160으로 종횡비를 유지하며 리사이징하여 WebP 포맷으로 제공하는 고성능 웹 서버입니다.

## 특징

- **고성능**: Rust + Axum으로 구현된 비동기 HTTP 서버
- **WebP 전용**: Discord CDN의 WebP 포맷을 지원하여 최적화된 이미지 처리
- **종횡비 유지**: 원본 이미지의 비율을 유지하면서 160x160 박스 내에서 최대 크기로 리사이징
- **캐싱**: 메모리 캐시(moka) + HTTP 캐시 헤더(ETag, Cache-Control)
- **최적화**: HTTP/2, 연결 재사용, keep-alive
- **컨테이너화**: Multi-stage Docker 빌드로 최소화된 이미지

## 빌드 및 실행

### Docker Compose (권장)

```bash
docker compose up --build
```

### Docker 직접 빌드

```bash
docker build -t emoji-resizer .
docker run --rm -p 8080:8080 emoji-resizer
```

### 로컬 개발 (Rust 설치 필요)

```bash
cargo run
```

## 사용법

서버가 실행되면 다음과 같이 사용할 수 있습니다:

```bash
# 건강 상태 확인
curl http://localhost:8080/healthz

# 이모지 리사이징 (예시 이모지 ID)
curl http://localhost:8080/e/123456789012345678.webp
```

## API 엔드포인트

- `GET /healthz` - 서버 건강 상태 확인
- `GET /e/:name` - 이모지 리사이징 및 제공
  - `:name`: Discord 이모지 파일명 (예: `123456789012345678.webp`)

## 환경변수

- `RUST_LOG`: 로그 레벨 설정 (기본값: `info`)
- `TOKIO_WORKER_THREADS`: Tokio 워커 스레드 수 (기본값: CPU 코어 수)

## 성능 최적화

1. **HTTP/2 + Keep-Alive**: 연결 재사용으로 지연시간 감소
2. **메모리 캐시**: 24시간 TTL로 자주 요청되는 이미지 캐시
3. **HTTP 캐시**: ETag와 Cache-Control로 브라우저/CDN 캐시 활용
4. **WebP 최적화**: Discord CDN의 WebP 포맷을 직접 처리하여 성능 향상
5. **Multi-stage 빌드**: 컨테이너 이미지 크기 최소화

## 프로덕션 배포

프로덕션 환경에서는 앞단에 Nginx나 Varnish 같은 리버스 프록시를 두어 디스크 캐시를 활용하는 것을 권장합니다.

## 한계사항

- 현재는 정적 이미지 및 애니메이션 WebP 지원 (Discord CDN 표준)
- WebP 출력만 지원 (Discord 환경에 최적화)
- 메모리 캐시만 지원 (디스크 캐시 미지원)
