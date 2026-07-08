//! Resumable download manager (issue #9).
//!
//! Downloads a URL to a destination path, supporting resume via HTTP
//! `Range` requests against a `.part` sidecar file. Progress is reported
//! through a caller-supplied callback so the HTTP/MCP API layer (later
//! issues) can stream it to clients.
//!
//! Checksum verification is deliberately *not* done here — that's #10's
//! job, layered on top of the file this module produces.

use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub bytes_downloaded: u64,
    /// `None` if the server didn't report a `Content-Length` (or
    /// equivalent) for the remaining bytes.
    pub total_bytes: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected HTTP status: {0}")]
    UnexpectedStatus(reqwest::StatusCode),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

fn part_path(dest: &Path) -> PathBuf {
    let mut os_string = dest.as_os_str().to_os_string();
    os_string.push(".part");
    PathBuf::from(os_string)
}

/// Downloads `url` to `dest_path`, resuming from a `.part` sidecar file if
/// one already exists from a previous interrupted attempt. On success, the
/// `.part` file is renamed to `dest_path`; on failure, it's left in place
/// so a subsequent call can resume.
///
/// `on_progress` is called after every chunk with the cumulative bytes
/// downloaded so far (including whatever was already on disk from a
/// previous attempt).
pub async fn download_resumable<F>(
    client: &reqwest::Client,
    url: &str,
    dest_path: &Path,
    mut on_progress: F,
) -> Result<(), DownloadError>
where
    F: FnMut(DownloadProgress),
{
    let part = part_path(dest_path);
    let existing_len = tokio::fs::metadata(&part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut request = client.get(url);
    if existing_len > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing_len}-"));
    }

    let response = request
        .send()
        .await
        .map_err(|e| DownloadError::Network(e.to_string()))?;
    let status = response.status();

    // Only treat this as a genuine resume if the server actually honored
    // the Range request (206). If we asked for a range and got a full 200
    // back instead (some servers/CDNs don't support Range), fall back to
    // downloading the whole thing from scratch rather than corrupting the
    // file by appending a full body onto existing bytes.
    let resuming = existing_len > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;

    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(DownloadError::UnexpectedStatus(status));
    }

    let mut downloaded = if resuming { existing_len } else { 0 };
    let total_bytes = response.content_length().map(|len| len + downloaded);

    let mut file = if resuming {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .await?
    } else {
        tokio::fs::File::create(&part).await?
    };

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| DownloadError::Network(e.to_string()))?;
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        on_progress(DownloadProgress {
            bytes_downloaded: downloaded,
            total_bytes,
        });
    }

    file.flush().await?;
    drop(file);
    tokio::fs::rename(&part, dest_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn downloads_full_file_from_scratch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world".as_slice()))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("file.bin");
        let client = reqwest::Client::new();
        let mut progress_calls = Vec::new();

        download_resumable(&client, &format!("{}/file.bin", server.uri()), &dest, |p| {
            progress_calls.push(p);
        })
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"hello world");
        assert!(!tokio::fs::try_exists(dest.with_extension("bin.part"))
            .await
            .unwrap());
        assert!(!progress_calls.is_empty());
        assert_eq!(
            progress_calls.last().unwrap().bytes_downloaded,
            "hello world".len() as u64
        );
    }

    #[tokio::test]
    async fn resumes_from_existing_part_file_via_range_request() {
        let server = MockServer::start().await;
        // Requiring the exact Range header proves resume behavior, not
        // just the final assembled content.
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .and(header("range", "bytes=5-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(b" world".as_slice()))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("file.bin");
        let mut part_path = dest.clone().into_os_string();
        part_path.push(".part");
        tokio::fs::write(&part_path, b"hello").await.unwrap();

        let client = reqwest::Client::new();
        download_resumable(
            &client,
            &format!("{}/file.bin", server.uri()),
            &dest,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn falls_back_to_full_restart_when_server_ignores_range() {
        let server = MockServer::start().await;
        // Server responds 200 (not 206) even though a Range was requested
        // -- we must not blindly append the full body onto the existing
        // partial bytes.
        Mock::given(method("GET"))
            .and(path("/file.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"hello world".as_slice()))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("file.bin");
        let mut part_path = dest.clone().into_os_string();
        part_path.push(".part");
        tokio::fs::write(&part_path, b"stale-partial-data")
            .await
            .unwrap();

        let client = reqwest::Client::new();
        download_resumable(
            &client,
            &format!("{}/file.bin", server.uri()),
            &dest,
            |_| {},
        )
        .await
        .unwrap();

        assert_eq!(tokio::fs::read(&dest).await.unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn http_error_status_returns_typed_error_and_leaves_no_dest_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing.bin"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("missing.bin");
        let client = reqwest::Client::new();

        let err = download_resumable(
            &client,
            &format!("{}/missing.bin", server.uri()),
            &dest,
            |_| {},
        )
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            DownloadError::UnexpectedStatus(reqwest::StatusCode::NOT_FOUND)
        ));
        assert!(!tokio::fs::try_exists(&dest).await.unwrap());
    }
}
