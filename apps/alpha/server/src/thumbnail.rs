//! Bounded, on-demand first frame from a verified server-owned video.
use super::*;
use axum::{body::Body, response::Response};
use std::sync::OnceLock;
use tokio::io::AsyncReadExt;

static DECODER: OnceLock<tokio::sync::Semaphore> = OnceLock::new();

pub async fn read(
    State(app): State<App>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Result<Response, ApiError> {
    auth(&app, &h)?;
    let guard = ReadGuard::new(&app);
    let _permit = tokio::time::timeout(
        Duration::from_secs(60),
        DECODER
            .get_or_init(|| tokio::sync::Semaphore::new(1))
            .acquire(),
    )
    .await
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "thumbnail_busy"))?
    .map_err(|_| error(StatusCode::SERVICE_UNAVAILABLE, "thumbnail_unavailable"))?;
    let source = tokio::task::spawn_blocking(move || {
        let store = attachment_bridge::store(&app).map_err(|_| "storage_unavailable")?;
        let (asset, file) = store.open_visible(&id)?;
        if asset.kind != "video" {
            return Err("thumbnail_not_video");
        }
        Ok(file)
    })
    .await
    .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage_unavailable"))?
    .map_err(|e| {
        error(
            if e == "asset_trashed" {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            },
            e,
        )
    })?;
    let _guard = guard;
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            "1",
            "-filter_threads",
            "1",
            "-protocol_whitelist",
            "pipe",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=640:-2:force_original_aspect_ratio=decrease",
            "-f",
            "image2pipe",
            "-vcodec",
            "mjpeg",
            "pipe:1",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    use std::os::unix::process::CommandExt;
    // No user code or allocation runs after fork; limits apply only to the decoder.
    unsafe {
        command.as_std_mut().pre_exec(|| {
            let memory = libc::rlimit {
                rlim_cur: 1_610_612_736,
                rlim_max: 1_610_612_736,
            };
            let cpu = libc::rlimit {
                rlim_cur: 15,
                rlim_max: 15,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0
                || libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| {
        error(
            StatusCode::SERVICE_UNAVAILABLE,
            "thumbnail_decoder_unavailable",
        )
    })?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "thumbnail_unavailable"))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| error(StatusCode::INTERNAL_SERVER_ERROR, "thumbnail_unavailable"))?;
    let writer = tokio::spawn(async move {
        let mut file = tokio::fs::File::from_std(source);
        let _ = tokio::io::copy(&mut file, &mut input).await;
    });
    let mut image = Vec::new();
    let decoded = tokio::time::timeout(Duration::from_secs(20), async {
        output
            .take(3 * 1024 * 1024 + 1)
            .read_to_end(&mut image)
            .await
            .map_err(|_| ())?;
        let status = child.wait().await.map_err(|_| ())?;
        if !status.success()
            || image.len() > 3 * 1024 * 1024
            || !image.starts_with(&[0xff, 0xd8, 0xff])
        {
            return Err(());
        }
        Ok(())
    })
    .await;
    writer.abort();
    if !matches!(decoded, Ok(Ok(()))) {
        let _ = child.kill().await;
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "thumbnail_unavailable",
        ));
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CACHE_CONTROL, "private, no-store")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(image))
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "thumbnail_unavailable"))
}
