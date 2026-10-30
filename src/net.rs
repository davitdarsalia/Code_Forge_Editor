use std::process::{Command, Stdio};

use futures::future::BoxFuture;
use http_client::http::HeaderValue;
use http_client::{AsyncBody, HttpClient, Method, Request, Response, StatusCode, Url};

const CURL: &str = "/usr/bin/curl";
const TIMEOUT_SECS: &str = "20";
const MAX_BYTES: &str = "33554432";

pub struct CurlClient;

pub fn split_status(mut output: Vec<u8>) -> Option<(u16, Vec<u8>)> {
    if output.len() < 3 {
        return None;
    }
    let status = std::str::from_utf8(&output[output.len() - 3..]).ok()?.parse::<u16>().ok()?;
    output.truncate(output.len() - 3);
    Some((status, output))
}

fn fetch(url: &str) -> anyhow::Result<Response<AsyncBody>> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        anyhow::bail!("unsupported url {url}");
    }
    let output = Command::new(CURL)
        .args(["-sSL", "--max-time", TIMEOUT_SECS, "--max-filesize", MAX_BYTES, "-A", "Forge", "-w", "%{http_code}", "--", url])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    let (status, body) = split_status(output.stdout).ok_or_else(|| anyhow::anyhow!("no response from {url}"))?;
    Ok(Response::builder().status(StatusCode::from_u16(status)?).body(AsyncBody::from(body))?)
}

impl HttpClient for CurlClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        None
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, req: Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let url = req.uri().to_string();
        let method = req.method().clone();
        Box::pin(async move {
            if method != Method::GET {
                anyhow::bail!("only GET is supported");
            }
            fetch(&url)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_split_from_the_body() {
        assert_eq!(split_status(b"<svg/>200".to_vec()), Some((200, b"<svg/>".to_vec())));
        assert_eq!(split_status(b"404".to_vec()), Some((404, Vec::new())));
        assert_eq!(split_status(b"ok".to_vec()), None);
        assert_eq!(split_status(b"bodyxyz".to_vec()), None);
    }
}
