use pixiv_sdk::fanbox::transport::Headers;
use std::time::{Duration, SystemTime};
use url::Url;
#[derive(Default)]
pub(super) struct Jar {
    cookies: Vec<Cookie>,
    sequence: u64,
}
struct Cookie {
    name: String,
    value: String,
    domain: String,
    host_only: bool,
    path: String,
    secure: bool,
    expiry: Option<SystemTime>,
    sequence: u64,
}
impl Jar {
    pub fn clearance(&mut self, url: &Url, value: &str) {
        self.set(url, &format!("cf_clearance={value}; Path=/"));
    }
    pub fn store(&mut self, url: &Url, headers: &Headers) {
        for (_, values) in headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("set-cookie"))
        {
            for value in values {
                self.set(url, value);
            }
        }
    }
    fn set(&mut self, url: &Url, raw: &str) {
        let mut parts = raw.split(';');
        let Some((name, value)) = parts.next().and_then(|pair| pair.split_once('=')) else {
            return;
        };
        let name = name.trim();
        let value = value.trim().trim_matches('"');
        if name.is_empty()
            || name
                .bytes()
                .any(|byte| byte <= 32 || byte >= 127 || b"()<>@,;:\\\"/[]?={}".contains(&byte))
        {
            return;
        }
        let host = url.host_str().unwrap_or("").to_ascii_lowercase();
        let mut domain = host.clone();
        let mut host_only = true;
        let mut path = url
            .path()
            .rsplit_once('/')
            .map(|(path, _)| if path.is_empty() { "/" } else { path })
            .unwrap_or("/")
            .to_owned();
        let mut secure = false;
        let mut expiry = None;
        let mut max_age = None;
        for part in parts {
            let (key, value) = part.trim().split_once('=').unwrap_or((part.trim(), ""));
            match key.to_ascii_lowercase().as_str() {
                "domain" => {
                    let candidate = value.trim_start_matches('.').to_ascii_lowercase();
                    if candidate.is_empty()
                        || !(host == candidate || host.ends_with(&format!(".{candidate}")))
                    {
                        return;
                    }
                    domain = candidate;
                    host_only = false;
                }
                "path" if value.starts_with('/') => path = value.to_owned(),
                "secure" => secure = true,
                "max-age" => max_age = value.parse::<i64>().ok(),
                "expires" => {
                    expiry = chrono::DateTime::parse_from_rfc2822(value)
                        .ok()
                        .and_then(|date| {
                            let seconds = date.timestamp();
                            if seconds >= 0 {
                                SystemTime::UNIX_EPOCH
                                    .checked_add(Duration::from_secs(seconds as u64))
                            } else {
                                Some(SystemTime::UNIX_EPOCH)
                            }
                        });
                }
                _ => {}
            }
        }
        if let Some(age) = max_age {
            expiry = if age <= 0 {
                Some(SystemTime::UNIX_EPOCH)
            } else {
                SystemTime::now().checked_add(Duration::from_secs(age as u64))
            };
        }
        let old = self.cookies.iter().position(|cookie| {
            cookie.name == name && cookie.domain == domain && cookie.path == path
        });
        let sequence = if let Some(index) = old {
            self.cookies.remove(index).sequence
        } else {
            self.sequence += 1;
            self.sequence
        };
        if expiry.is_some_and(|expiry| expiry <= SystemTime::now()) {
            return;
        }
        self.cookies.push(Cookie {
            name: name.to_owned(),
            value: value.to_owned(),
            domain,
            host_only,
            path,
            secure,
            expiry,
            sequence,
        });
    }
    pub fn header(&mut self, url: &Url) -> String {
        self.cookies.retain(|cookie| {
            cookie
                .expiry
                .is_none_or(|expiry| expiry > SystemTime::now())
        });
        let host = url.host_str().unwrap_or("");
        let path = url.path();
        let mut selected: Vec<_> = self
            .cookies
            .iter()
            .filter(|cookie| {
                (host == cookie.domain
                    || (!cookie.host_only && host.ends_with(&format!(".{}", cookie.domain))))
                    && (!cookie.secure || url.scheme() == "https")
                    && (path == cookie.path
                        || (path.starts_with(&cookie.path)
                            && (cookie.path.ends_with('/')
                                || path[cookie.path.len()..].starts_with('/'))))
            })
            .collect();
        selected.sort_by(|left, right| {
            right
                .path
                .len()
                .cmp(&left.path.len())
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        selected
            .iter()
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ")
    }
}
