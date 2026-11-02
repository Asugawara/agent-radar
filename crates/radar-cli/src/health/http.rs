//! HTTP checks: classify one URL (or GitHub repo) as ok / dead / undecided.

use std::collections::HashMap;
use std::error::Error as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use url::Url;

use super::state::Outcome;
use crate::github;

pub const MAX_CONCURRENCY: usize = 16;
pub const MAX_PER_HOST: usize = 2;
pub const TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_REDIRECTS: usize = 10;

/// Redirecting to one of these (from another domain) means the domain is for
/// sale or parked. Matched as host suffix on a label boundary.
pub const PARKING_HOSTS: &[&str] = &[
    "sedo.com",
    "sedoparking.com",
    "afternic.com",
    "dan.com",
    "hugedomains.com",
    "parkingcrew.net",
    "bodis.com",
    "undeveloped.com",
    "forsale.godaddy.com",
];

/// Not checked at all (they reject bots).
pub const UNCHECKED_HOSTS: &[&str] = &["x.com", "twitter.com"];

fn host_matches(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

/// godaddy's "for sale" landing pages live on several hosts; those are
/// recognised by path.
pub fn is_parking_url(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    PARKING_HOSTS.iter().any(|p| host_matches(host, p))
        || (host_matches(host, "godaddy.com")
            && url.path().to_ascii_lowercase().contains("forsale"))
}

/// Discord invite code of `discord.gg/<code>` / `discord.com/invite/<code>`.
pub fn discord_invite(url: &Url) -> Option<String> {
    let host = url.host_str()?;
    let mut segs = url.path_segments()?.filter(|s| !s.is_empty());
    let first = segs.next()?;
    let code = if host_matches(host, "discord.gg") {
        first
    } else if host_matches(host, "discord.com") && first == "invite" {
        segs.next()?
    } else {
        return None;
    };
    Some(code.to_string())
}

#[derive(Clone)]
pub struct Config {
    pub timeout: Duration,
    pub max_concurrency: usize,
    pub max_per_host: usize,
    pub discord_api: String,
    pub github_api: String,
    pub github_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            timeout: TIMEOUT,
            max_concurrency: MAX_CONCURRENCY,
            max_per_host: MAX_PER_HOST,
            discord_api: "https://discord.com/api/v10".into(),
            github_api: github::API_BASE.into(),
            github_token: std::env::var("GITHUB_TOKEN").ok().filter(|t| !t.is_empty()),
        }
    }
}

pub struct Checker {
    client: Client,
    cfg: Config,
    global: Arc<tokio::sync::Semaphore>,
    hosts: Mutex<HashMap<String, Arc<tokio::sync::Semaphore>>>,
}

impl Checker {
    pub fn new(cfg: Config) -> anyhow::Result<Self> {
        let policy = Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("too many redirects");
            }
            let origin = attempt.previous().first().and_then(|u| u.host_str());
            let parked = is_parking_url(attempt.url()) && attempt.url().host_str() != origin;
            if parked {
                // Do not follow: the 3xx response is classified as dead.
                attempt.stop()
            } else {
                attempt.follow()
            }
        });
        let client = Client::builder()
            .user_agent(github::user_agent())
            .timeout(cfg.timeout)
            .redirect(policy)
            .build()?;
        Ok(Checker {
            client,
            global: Arc::new(tokio::sync::Semaphore::new(cfg.max_concurrency)),
            hosts: Mutex::new(HashMap::new()),
            cfg,
        })
    }

    fn host_semaphore(&self, host: &str) -> Arc<tokio::sync::Semaphore> {
        let mut hosts = self.hosts.lock().unwrap();
        hosts
            .entry(host.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(self.cfg.max_per_host)))
            .clone()
    }

    /// Check a web URL, respecting the concurrency limits.
    pub async fn check_url(&self, raw: &str) -> Outcome {
        let Ok(url) = Url::parse(raw) else {
            return Outcome::Undecided("invalid URL".into());
        };
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
            return Outcome::Undecided("no host".into());
        };
        if UNCHECKED_HOSTS.iter().any(|h| host_matches(&host, h)) {
            return Outcome::Unchecked;
        }
        // Discord invites go to the API, whose host differs from the link's.
        let limit_host = if discord_invite(&url).is_some() {
            "discord-api".to_string()
        } else {
            host
        };
        let _host_permit = self.host_semaphore(&limit_host).acquire_owned().await;
        let _permit = self.global.acquire().await;
        if let Some(code) = discord_invite(&url) {
            return self.check_discord(&code).await;
        }
        self.check_web(&url).await
    }

    async fn check_discord(&self, code: &str) -> Outcome {
        let api = format!("{}/invites/{code}", self.cfg.discord_api);
        match self.client.get(api).send().await {
            Ok(r) if r.status() == StatusCode::NOT_FOUND => {
                Outcome::Dead("discord invite expired".into())
            }
            Ok(r) if r.status().is_success() => Outcome::Ok,
            other => classify(other),
        }
    }

    async fn check_web(&self, url: &Url) -> Outcome {
        let head = classify(self.client.head(url.clone()).send().await);
        match &head {
            Outcome::Ok => return head,
            // DNS failures and parked domains do not change with GET.
            Outcome::Dead(r) if r == "dns" || r.starts_with("parked") => return head,
            _ => {}
        }
        // Any other HEAD result (405, 403, 404, errors, ...) is confirmed with GET.
        classify(self.client.get(url.clone()).send().await)
    }

    /// Check that `owner/name` exists, through the GitHub API.
    pub async fn check_repo(&self, repo: &str) -> Outcome {
        if github::split_repo(repo).is_none() {
            return Outcome::Undecided("invalid repo".into());
        }
        let _host_permit = self.host_semaphore("github-api").acquire_owned().await;
        let _permit = self.global.acquire().await;
        let mut req = self
            .client
            .get(format!("{}/repos/{repo}", self.cfg.github_api))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(t) = &self.cfg.github_token {
            req = req.bearer_auth(t);
        }
        match req.send().await {
            Ok(r) if r.status() == StatusCode::NOT_FOUND => Outcome::Dead("repo 404".into()),
            Ok(r) if r.status().is_success() => Outcome::Ok,
            other => classify(other),
        }
    }
}

fn is_dns_error(e: &reqwest::Error) -> bool {
    let mut msgs = vec![e.to_string()];
    let mut src = e.source();
    while let Some(s) = src {
        msgs.push(s.to_string());
        src = s.source();
    }
    msgs.iter().any(|m| {
        let m = m.to_ascii_lowercase();
        m.contains("dns error")
            || m.contains("failed to lookup address")
            || m.contains("no such host")
            || m.contains("name or service not known")
            || m.contains("nodename nor servname")
    })
}

/// Map a response or transport error to an [`Outcome`].
pub fn classify(res: Result<Response, reqwest::Error>) -> Outcome {
    match res {
        Ok(r) => {
            let status = r.status();
            if status.is_success() {
                return Outcome::Ok;
            }
            if status.is_redirection()
                && let Some(loc) = r.headers().get("location").and_then(|v| v.to_str().ok())
                && let Ok(target) = r.url().join(loc)
                && is_parking_url(&target)
            {
                return Outcome::Dead(format!("parked: {}", target.host_str().unwrap_or("?")));
            }
            match status {
                StatusCode::NOT_FOUND => Outcome::Dead("404".into()),
                StatusCode::GONE => Outcome::Dead("410".into()),
                s => Outcome::Undecided(s.as_u16().to_string()),
            }
        }
        Err(e) if e.is_timeout() => Outcome::Undecided("timeout".into()),
        Err(e) if is_dns_error(&e) => Outcome::Dead("dns".into()),
        Err(e) if e.is_redirect() => Outcome::Undecided("redirect loop".into()),
        Err(e) => {
            let detail = e
                .source()
                .map(|s| s.to_string())
                .unwrap_or_else(|| e.to_string());
            Outcome::Undecided(format!("error: {detail}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(server: &MockServer) -> Config {
        Config {
            timeout: Duration::from_millis(400),
            discord_api: format!("{}/api/v10", server.uri()),
            github_api: server.uri(),
            github_token: None,
            ..Config::default()
        }
    }

    async fn check(server: &MockServer, p: &str) -> Outcome {
        Checker::new(cfg(server))
            .unwrap()
            .check_url(&format!("{}{p}", server.uri()))
            .await
    }

    fn status(code: u16) -> ResponseTemplate {
        ResponseTemplate::new(code)
    }

    #[tokio::test]
    async fn ok_404_410() {
        let s = MockServer::start().await;
        Mock::given(path("/ok"))
            .respond_with(status(200))
            .mount(&s)
            .await;
        Mock::given(path("/gone"))
            .respond_with(status(410))
            .mount(&s)
            .await;
        Mock::given(path("/missing"))
            .respond_with(status(404))
            .mount(&s)
            .await;
        assert_eq!(check(&s, "/ok").await, Outcome::Ok);
        assert_eq!(check(&s, "/gone").await, Outcome::Dead("410".into()));
        assert_eq!(check(&s, "/missing").await, Outcome::Dead("404".into()));
    }

    #[tokio::test]
    async fn blocked_and_failing_sites_are_undecided() {
        let s = MockServer::start().await;
        for code in [401, 403, 429, 500, 503] {
            Mock::given(path(format!("/{code}")))
                .respond_with(status(code))
                .mount(&s)
                .await;
            assert_eq!(
                check(&s, &format!("/{code}")).await,
                Outcome::Undecided(code.to_string()),
                "{code}"
            );
        }
    }

    #[tokio::test]
    async fn head_405_falls_back_to_get() {
        let s = MockServer::start().await;
        Mock::given(method("HEAD"))
            .and(path("/p"))
            .respond_with(status(405))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/p"))
            .respond_with(status(200))
            .mount(&s)
            .await;
        assert_eq!(check(&s, "/p").await, Outcome::Ok);
    }

    #[tokio::test]
    async fn head_403_and_404_are_confirmed_with_get() {
        let s = MockServer::start().await;
        for (p, head) in [("/a", 403), ("/b", 404)] {
            Mock::given(method("HEAD"))
                .and(path(p))
                .respond_with(status(head))
                .mount(&s)
                .await;
            Mock::given(method("GET"))
                .and(path(p))
                .respond_with(status(200))
                .mount(&s)
                .await;
        }
        assert_eq!(check(&s, "/a").await, Outcome::Ok);
        assert_eq!(check(&s, "/b").await, Outcome::Ok);
        Mock::given(method("HEAD"))
            .and(path("/c"))
            .respond_with(status(404))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/c"))
            .respond_with(status(404))
            .mount(&s)
            .await;
        assert_eq!(check(&s, "/c").await, Outcome::Dead("404".into()));
    }

    #[tokio::test]
    async fn redirect_to_a_parking_host_is_dead() {
        let s = MockServer::start().await;
        Mock::given(path("/sale"))
            .respond_with(
                status(302).insert_header("location", "https://www.sedo.com/search/?domain=x.dev"),
            )
            .mount(&s)
            .await;
        Mock::given(path("/godaddy"))
            .respond_with(
                status(301).insert_header("location", "https://www.godaddy.com/forsale/x.dev"),
            )
            .mount(&s)
            .await;
        Mock::given(path("/inner"))
            .respond_with(status(302).insert_header("location", "/ok"))
            .mount(&s)
            .await;
        Mock::given(path("/ok"))
            .respond_with(status(200))
            .mount(&s)
            .await;
        assert_eq!(
            check(&s, "/sale").await,
            Outcome::Dead("parked: www.sedo.com".into())
        );
        assert_eq!(
            check(&s, "/godaddy").await,
            Outcome::Dead("parked: www.godaddy.com".into())
        );
        assert_eq!(check(&s, "/inner").await, Outcome::Ok);
    }

    #[tokio::test]
    async fn timeout_is_undecided() {
        let s = MockServer::start().await;
        Mock::given(path("/slow"))
            .respond_with(status(200).set_delay(Duration::from_secs(3)))
            .mount(&s)
            .await;
        assert_eq!(
            check(&s, "/slow").await,
            Outcome::Undecided("timeout".into())
        );
    }

    #[tokio::test]
    async fn redirect_loop_is_undecided() {
        let s = MockServer::start().await;
        Mock::given(path("/loop"))
            .respond_with(status(302).insert_header("location", "/loop"))
            .mount(&s)
            .await;
        assert!(matches!(check(&s, "/loop").await, Outcome::Undecided(_)));
    }

    #[tokio::test]
    async fn twitter_is_not_checked() {
        let s = MockServer::start().await;
        let c = Checker::new(cfg(&s)).unwrap();
        assert_eq!(
            c.check_url("https://x.com/someone").await,
            Outcome::Unchecked
        );
        assert_eq!(
            c.check_url("https://www.twitter.com/someone").await,
            Outcome::Unchecked
        );
    }

    #[tokio::test]
    async fn discord_invites_use_the_api() {
        let s = MockServer::start().await;
        Mock::given(path("/api/v10/invites/alive"))
            .respond_with(status(200))
            .mount(&s)
            .await;
        Mock::given(path("/api/v10/invites/expired"))
            .respond_with(status(404))
            .mount(&s)
            .await;
        Mock::given(path("/api/v10/invites/limited"))
            .respond_with(status(429))
            .mount(&s)
            .await;
        let c = Checker::new(cfg(&s)).unwrap();
        assert_eq!(c.check_url("https://discord.gg/alive").await, Outcome::Ok);
        assert_eq!(
            c.check_url("https://discord.com/invite/expired").await,
            Outcome::Dead("discord invite expired".into())
        );
        assert_eq!(
            c.check_url("https://discord.gg/limited").await,
            Outcome::Undecided("429".into())
        );
    }

    #[tokio::test]
    async fn repo_404_is_dead() {
        let s = MockServer::start().await;
        Mock::given(path("/repos/o/alive"))
            .respond_with(status(200))
            .mount(&s)
            .await;
        Mock::given(path("/repos/o/gone"))
            .respond_with(status(404))
            .mount(&s)
            .await;
        Mock::given(path("/repos/o/limited"))
            .respond_with(status(403))
            .mount(&s)
            .await;
        let c = Checker::new(cfg(&s)).unwrap();
        assert_eq!(c.check_repo("o/alive").await, Outcome::Ok);
        assert_eq!(
            c.check_repo("o/gone").await,
            Outcome::Dead("repo 404".into())
        );
        assert_eq!(
            c.check_repo("o/limited").await,
            Outcome::Undecided("403".into())
        );
    }

    #[tokio::test]
    async fn unresolvable_host_is_dead() {
        // `.invalid` never resolves (RFC 6761), no real site is contacted.
        let c = Checker::new(Config {
            timeout: Duration::from_secs(5),
            ..Config::default()
        })
        .unwrap();
        let out = c.check_url("http://agent-radar-test.invalid/").await;
        assert_eq!(out, Outcome::Dead("dns".into()), "{out:?}");
    }

    #[test]
    fn discord_and_parking_parsing() {
        let u = |s: &str| Url::parse(s).unwrap();
        assert_eq!(
            discord_invite(&u("https://discord.gg/abc")).as_deref(),
            Some("abc")
        );
        assert_eq!(
            discord_invite(&u("https://discord.com/invite/xyz/")).as_deref(),
            Some("xyz")
        );
        assert_eq!(discord_invite(&u("https://discord.com/channels/1/2")), None);
        assert_eq!(discord_invite(&u("https://example.com/abc")), None);
        assert!(is_parking_url(&u("https://dan.com/buy-domain/x.dev")));
        assert!(is_parking_url(&u(
            "https://www.hugedomains.com/domain_profile.cfm?d=x"
        )));
        assert!(!is_parking_url(&u("https://notdan.com/")));
        assert!(!is_parking_url(&u("https://www.godaddy.com/about")));
    }
}
