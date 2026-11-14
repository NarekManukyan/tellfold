//! Tellfold's usage numbers: a Cloudflare Worker that counts update checks from the Mac app and visits to the site.
//!
//! - `/appcast.xml`: Sparkle in the app asks for this about once a day. The Worker passes the request on to the real
//!   feed on GitHub Pages and adds one to a counter for that day and app version, read from Sparkle's User-Agent
//!   (`Tellfold/0.7.0 Sparkle/2.10.1`). Requests that don't come from the app get the feed but aren't counted.
//! - `/hit?n=<page or link>&s=<source>`: the site's script reports a page view or a Download/Sponsor click here.
//!   Only requests from the site itself are counted.
//!
//! Each count is one row per day and label. No IP address, identifier, cookie or other header is kept.

use worker::*;

/// The real feed. `release.sh` keeps writing it; this Worker only relays it.
const UPSTREAM: &str = "https://narekmanukyan.github.io/tellfold/appcast.xml";
const SITE: &str = "https://narekmanukyan.github.io/tellfold/";
const SITE_ORIGIN: &str = "https://narekmanukyan.github.io";

#[event(fetch)]
async fn fetch(req: Request, env: Env, ctx: Context) -> Result<Response> {
    match req.path().as_str() {
        "/appcast.xml" => appcast(req, env, ctx).await,
        "/hit" => hit(req, env, ctx),
        _ => Response::redirect(Url::parse(SITE)?),
    }
}

async fn appcast(req: Request, env: Env, ctx: Context) -> Result<Response> {
    if !matches!(req.method(), Method::Get | Method::Head) {
        return Response::error("Method not allowed", 405);
    }
    let agent = req.headers().get("User-Agent")?.unwrap_or_default();
    if req.method() == Method::Get {
        if let Some(version) = app_version(&agent) {
            let db = env.d1("DB")?;
            // Counting never holds up or breaks the update check.
            ctx.wait_until(async move {
                let insert = "INSERT INTO checks (day, version, n) VALUES (date('now'), ?1, 1)
                              ON CONFLICT (day, version) DO UPDATE SET n = n + 1";
                if let Err(err) = run(&db, insert, &[version]).await {
                    console_error!("counting an update check failed: {err}");
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

fn hit(req: Request, env: Env, ctx: Context) -> Result<Response> {
    let headers = req.headers();
    let origin = headers.get("Origin")?.unwrap_or_default();
    let agent = headers.get("User-Agent")?.unwrap_or_default();
    let url = req.url()?;
    let param = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());
    let counted = origin == SITE_ORIGIN && !looks_automated(&agent);
    if let (true, Some(name), source) = (counted, param("n").and_then(|n| label(&n)), param("s")) {
        let source = source.and_then(|s| label(&s)).unwrap_or_else(|| "direct".into());
        let db = env.d1("DB")?;
        ctx.wait_until(async move {
            let insert = "INSERT INTO visits (day, name, source, n) VALUES (date('now'), ?1, ?2, 1)
                          ON CONFLICT (day, name, source) DO UPDATE SET n = n + 1";
            if let Err(err) = run(&db, insert, &[name, source]).await {
                console_error!("counting a visit failed: {err}");
            }
        });
    }
    // The same empty answer whether or not it counted, so the site never waits on or learns anything from it.
    Ok(Response::empty()?.with_status(204))
}

async fn run(db: &D1Database, sql: &str, values: &[String]) -> Result<()> {
    let values: Vec<_> = values.iter().map(|v| v.as_str().into()).collect();
    db.prepare(sql).bind(&values)?.run().await?;
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

/// A page path, link name or source as a short lowercase label, or `None` if it isn't one. Keeps the table small and
/// free of anything that could carry personal data.
pub fn label(raw: &str) -> Option<String> {
    let label = raw.trim().to_ascii_lowercase();
    let ok = !label.is_empty()
        && label.len() <= 64
        && label.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '-' | '_'));
    ok.then_some(label)
}

/// Crawlers and headless browsers that run the site's script.
pub fn looks_automated(agent: &str) -> bool {
    let agent = agent.to_ascii_lowercase();
    agent.is_empty()
        || ["bot", "crawl", "spider", "slurp", "headless", "lighthouse", "preview", "python", "curl"]
            .iter()
            .any(|word| agent.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_from_sparkle() {
        assert_eq!(app_version("Tellfold/0.7.0 Sparkle/2.10.1").as_deref(), Some("0.7.0"));
        assert_eq!(app_version("Tellfold/0.7.1 (Macintosh) Sparkle/2.10.1").as_deref(), Some("0.7.1"));
    }

    #[test]
    fn ignores_other_feed_readers() {
        assert_eq!(app_version(""), None);
        assert_eq!(app_version("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15"), None);
        assert_eq!(app_version("Tellfold/0.7.0"), None);
        assert_eq!(app_version("curl/8.7.1 Sparkle/2.10.1"), None);
        assert_eq!(app_version("Tellfold/<script> Sparkle/2.10.1"), None);
        assert_eq!(app_version("Tellfold/1.2.3.4.5.6.7.8.9.10 Sparkle/2"), None);
    }

    #[test]
    fn keeps_labels_short_and_plain() {
        assert_eq!(label("/tellfold/").as_deref(), Some("/tellfold/"));
        assert_eq!(label("Download-Hero").as_deref(), Some("download-hero"));
        assert_eq!(label("www.linkedin.com").as_deref(), Some("www.linkedin.com"));
        assert_eq!(label(""), None);
        assert_eq!(label("jane@example.com"), None);
        assert_eq!(label("/tellfold/?email=x"), None);
        assert_eq!(label(&"a".repeat(65)), None);
    }

    #[test]
    fn spots_crawlers() {
        assert!(looks_automated("Mozilla/5.0 (compatible; Googlebot/2.1)"));
        assert!(looks_automated("Mozilla/5.0 HeadlessChrome/131.0"));
        assert!(looks_automated(""));
        assert!(!looks_automated("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15"));
    }
}
