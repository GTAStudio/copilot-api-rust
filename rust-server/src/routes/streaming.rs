use axum::body::Body;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::Response;
use bytes::Bytes;
use futures::{Stream, StreamExt};

pub fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, "text/event-stream".parse().unwrap());
    headers.insert(CACHE_CONTROL, "no-cache".parse().unwrap());
    headers.insert("x-accel-buffering", "no".parse().unwrap());
    response
}

pub fn json_events(
    response: reqwest::Response,
) -> impl Stream<Item = Result<serde_json::Value, std::io::Error>> + Send {
    async_stream::stream! {
        let stream = response.bytes_stream();
        futures::pin_mut!(stream);
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => { yield Err(std::io::Error::other(error)); return; }
            };
            if buffer.len().saturating_add(chunk.len()) > 16 * 1024 * 1024 {
                yield Err(std::io::Error::other("Upstream SSE event exceeds buffer limit"));
                return;
            }
            buffer.extend_from_slice(&chunk);
            for block in drain_sse_blocks(&mut buffer) {
                let Some(data) = extract_sse_data(&block) else { continue; };
                if data.trim() == "[DONE]" { return; }
                if data.trim().is_empty() { continue; }
                match serde_json::from_str::<serde_json::Value>(&data) {
                    Ok(event) if event.get("error").is_some() || event["type"] == "error" => {
                        yield Err(std::io::Error::other("Upstream stream reported an error")); return;
                    }
                    Ok(event) => yield Ok(event),
                    Err(_) => { yield Err(std::io::Error::other("Invalid upstream SSE JSON")); return; }
                }
            }
        }
        if buffer.iter().any(|byte| !byte.is_ascii_whitespace()) {
            yield Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Incomplete upstream SSE event"));
        }
    }
}

fn find_event_end(buffer: &[u8]) -> Option<usize> {
    let mut index = 0;
    let mut line_start = 0;
    while index < buffer.len() {
        if matches!(buffer[index], b'\r' | b'\n') {
            let width = if buffer[index..].starts_with(b"\r\n") {
                2
            } else {
                1
            };
            if index == line_start {
                return Some(index + width);
            }
            index += width;
            line_start = index;
        } else {
            index += 1;
        }
    }
    None
}

pub fn drain_sse_blocks(buffer: &mut Vec<u8>) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut consumed = 0;
    while let Some(end) = find_event_end(&buffer[consumed..]) {
        blocks.push(String::from_utf8_lossy(&buffer[consumed..consumed + end]).into_owned());
        consumed += end;
    }
    buffer.drain(..consumed);
    blocks
}

pub fn extract_sse_data(block: &str) -> Option<String> {
    let lines: Vec<&str> = block
        .trim_start_matches('\u{feff}')
        .split(['\r', '\n'])
        .filter_map(|line| {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            (field == "data").then(|| value.strip_prefix(' ').unwrap_or(value))
        })
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::sse_response;
    use bytes::Bytes;
    use futures::stream;

    #[test]
    fn sets_sse_headers() {
        let stream = stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(
            b"data: test\n\n",
        ))]);
        let resp = sse_response(stream);
        let headers = resp.headers();
        assert_eq!(
            headers.get("content-type").and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            headers.get("cache-control").and_then(|v| v.to_str().ok()),
            Some("no-cache")
        );
        assert!(!headers.contains_key("connection"));
        assert_eq!(headers["x-accel-buffering"], "no");
    }
}
