use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::Stream;
use axum::http::header::{CACHE_CONTROL, CONNECTION, CONTENT_TYPE};

pub fn sse_response<S>(stream: S) -> Response
where
    S: Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
{
    let body = Body::from_stream(stream);
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, "text/event-stream".parse().unwrap());
    headers.insert(CACHE_CONTROL, "no-cache".parse().unwrap());
    headers.insert(CONNECTION, "keep-alive".parse().unwrap());
    response
}

#[cfg(test)]
mod tests {
    use super::sse_response;
    use bytes::Bytes;
    use futures::stream;

    #[test]
    fn sets_sse_headers() {
        let stream = stream::iter(vec![Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: test\n\n"))]);
        let resp = sse_response(stream);
        let headers = resp.headers();
        assert_eq!(headers.get("content-type").and_then(|v| v.to_str().ok()), Some("text/event-stream"));
        assert_eq!(headers.get("cache-control").and_then(|v| v.to_str().ok()), Some("no-cache"));
        assert_eq!(headers.get("connection").and_then(|v| v.to_str().ok()), Some("keep-alive"));
    }
}

