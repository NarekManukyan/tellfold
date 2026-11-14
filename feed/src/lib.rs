//! The Mac app's update feed, served from Cloudflare so the daily update checks can be counted.
//!
//! Sparkle in the app asks for `/appcast.xml` about once a day. This Worker passes the request on to the real feed on
//! GitHub Pages and adds one to a counter for that day and app version. It reads the version from Sparkle's
//! User-Agent (`Tellfold/0.7.0 Sparkle/2.10.1`) and stores nothing else: no IP address, no identifier, no other
//! header. Requests that don't come from the app (a browser, a crawler) get the feed but aren't counted.

use worker::*;

/// The real feed. `release.sh` keeps writing it; this Worker only relays it.
const UPSTREAM: &str = "https://narekmanukyan.github.io/tellfold/appcast.xml";
const SITE: &str = "https://narekmanukyan.github.io/tellfold/";

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if req.path() != "/appcast.xml" {
        return Response::redirect(Url::parse(SITE)?);
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return Response::error("Method not allowed", 405);
    }

    let agent = req.headers().get("User-Agent")?.unwrap_or_default();
    if req.method() == Method::Get {
        if let Some(version) = app_version(&agent) {
            let db = env.d1("DB")?;
            // Counting never holds up or breaks the update check.
            ctx.wait_until(async move {
                if let Err(err) = count(&db, &version).await {
                    console_error!("count failed: {err}");
                }
            });
        }
    }

    feed(req.method()).await
}

/// Fetches the feed from GitHub Pages, cached at Cloudflare's edge for five minutes so a release shows up quickly.
async fn feed(method: Method) -> Result<Response> {
    let mut init = RequestInit::new();
    init.cf = CfProperties {
        cache_ttl: Some(300),
        cache_everything: Some(true),
        ..CfProperties::default()
    };
    let mut upstream = Fetch::Request(Request::new_with_init(UPSTREAM, &init)?).send().await?;
    if upstream.status_code() != 200 {
        return Response::error("The update feed is unavailable", 502);
    }
    let body = upstream.bytes().await?;
    let headers = Headers::new();
    headers.set("Content-Type", "application/rss+xml; charset=utf-8")?;
    headers.set("Cache-Control", "no-cache")?;
    let response = if method == Method::Head { Response::empty()? } else { Response::from_bytes(body)? };
    Ok(response.with_headers(headers))
}

async fn count(db: &D1Database, version: &str) -> Result<()> {
    db.prepare(
        "INSERT INTO checks (day, version, n) VALUES (date('now'), ?1, 1)
         ON CONFLICT (day, version) DO UPDATE SET n = n + 1",
    )
    .bind(&[version.into()])?
    .run()
    .await?;
    Ok(())
}

/// The app version from Sparkle's User-Agent, or `None` when the request isn't from the app.
pub fn app_version(agent: &str) -> Option<String> {
    let mut words = agent.split_whitespace();
    let version = words.next()?.strip_prefix("Tellfold/")?;
    if !words.any(|word| word.starts_with("Sparkle/")) {
        return None;
    }
    let plausible = !version.is_empty()
        && version.len() <= 16
        && version.chars().all(|c| c.is_ascii_digit() || c == '.');
    plausible.then(|| version.to_string())
}

#[cfg(test)]
mod tests {
    use super::app_version;

    #[test]
    fn reads_the_version_from_sparkle() {
        assert_eq!(app_version("Tellfold/0.7.0 Sparkle/2.10.1").as_deref(), Some("0.7.0"));
        assert_eq!(app_version("Tellfold/0.7.1 (Macintosh) Sparkle/2.10.1").as_deref(), Some("0.7.1"));
    }

    #[test]
    fn ignores_everyone_else() {
        assert_eq!(app_version(""), None);
        assert_eq!(app_version("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15"), None);
        assert_eq!(app_version("Tellfold/0.7.0"), None);
        assert_eq!(app_version("curl/8.7.1 Sparkle/2.10.1"), None);
        assert_eq!(app_version("Tellfold/<script> Sparkle/2.10.1"), None);
        assert_eq!(app_version("Tellfold/1.2.3.4.5.6.7.8.9.10 Sparkle/2"), None);
    }
}
