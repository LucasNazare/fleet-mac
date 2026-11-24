//! Presence: while a session is open on a Mac, GitHub has refs/fleet/presence/<branch>
//! pointing at a tiny tag object (machine name, install id, time). It's a ref, not a file,
//! so refreshing it doesn't add commits. Pushes carry a lease on it, so two Macs can never
//! push the same branch at the same time.
use super::gitx::{GResult, Git};

pub const FRESH_MS: i64 = 10 * 60 * 1000;
pub const REFRESH_MS: i64 = 5 * 60 * 1000;

pub fn remote_ref(branch: &str) -> String {
    format!("refs/fleet/presence/{branch}")
}

pub fn tracking_ref(branch: &str) -> String {
    format!("refs/fleet/rpresence/{branch}")
}

/// Fetch refspec that mirrors every presence ref (prunable).
pub const FETCH_SPEC: &str = "+refs/fleet/presence/*:refs/fleet/rpresence/*";

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub machine: String,
    pub install: String,
    pub at_ms: i64,
    pub sha: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Holder {
    Nobody,
    Mine,
    Other { machine: String, age_ms: i64 },
    /// Someone else's, older than 10 minutes: free to take.
    Stale { machine: String, age_ms: i64 },
}

pub fn holder(p: Option<&Presence>, me: &str, now_ms: i64) -> Holder {
    match p {
        None => Holder::Nobody,
        Some(p) if p.install == me => Holder::Mine,
        Some(p) => {
            let age = (now_ms - p.at_ms).max(0);
            if age < FRESH_MS {
                Holder::Other { machine: p.machine.clone(), age_ms: age }
            } else {
                Holder::Stale { machine: p.machine.clone(), age_ms: age }
            }
        }
    }
}

pub fn ago(ms: i64) -> String {
    let m = ms / 60_000;
    if m < 1 {
        "just now".into()
    } else if m < 60 {
        format!("{m}m ago")
    } else {
        format!("{}h ago", m / 60)
    }
}

/// A tag object for `head` saying "open on <machine>". Not referenced until pushed.
pub fn make_tag(git: &Git, head: &str, machine: &str, install: &str, now_secs: i64) -> GResult<String> {
    let machine = machine.replace(['\n', '\r'], " ");
    let body = format!("object {head}\ntype commit\ntag fleet-presence\ntagger fleet <fleet@presence> {now_secs} +0000\n\n{machine}\nfleet-install: {install}\n");
    Ok(git.run_with(&["mktag"], &[], Some(body.as_bytes()), false)?.trim().to_string())
}

pub fn parse_tag(text: &str) -> Option<(String, String, i64)> {
    let (head, body) = text.split_once("\n\n")?;
    let at = head.lines().find_map(|l| l.strip_prefix("tagger ")).and_then(|t| t.rsplit(' ').nth(1)).and_then(|s| s.parse::<i64>().ok())?;
    let mut lines = body.lines();
    let machine = lines.next()?.trim().to_string();
    let install = body.lines().find_map(|l| l.strip_prefix("fleet-install: ")).unwrap_or("").trim().to_string();
    Some((machine, install, at * 1000))
}

pub fn read(git: &Git, sha: &str) -> Option<Presence> {
    let text = git.read(&["cat-file", "tag", sha]).ok()?;
    let (machine, install, at_ms) = parse_tag(&text)?;
    Some(Presence { machine, install, at_ms, sha: sha.to_string() })
}

/// The presence ref of a branch as last fetched.
pub fn fetched(git: &Git, branch: &str) -> Option<Presence> {
    let sha = git.read(&["rev-parse", "--verify", "-q", &tracking_ref(branch)]).ok()?.trim().to_string();
    read(git, &sha)
}

/// Fetch one branch's presence now (before deciding to take it over).
pub fn fetch_one(git: &Git, branch: &str) -> Option<Presence> {
    let _ = git.read(&["update-ref", "-d", &tracking_ref(branch)]);
    let _ = git.run(&["fetch", "-q", "origin", &format!("+{}:{}", remote_ref(branch), tracking_ref(branch))]);
    fetched(git, branch)
}

/// All fetched presences: branch → presence.
pub fn all(git: &Git) -> Vec<(String, Presence)> {
    let list = git.read(&["for-each-ref", "--format=%(refname) %(objectname)", "refs/fleet/rpresence/"]).unwrap_or_default();
    list.lines()
        .filter_map(|l| {
            let (r, sha) = l.split_once(' ')?;
            let branch = r.strip_prefix("refs/fleet/rpresence/")?.to_string();
            Some((branch, read(git, sha)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::gitx::testutil::Fixture;
    use super::*;

    #[test]
    fn holder_rules() {
        let p = |install: &str, at: i64| Presence { machine: "iMac".into(), install: install.into(), at_ms: at, sha: "x".into() };
        let now = 100 * 60_000;
        assert_eq!(holder(None, "me", now), Holder::Nobody);
        assert_eq!(holder(Some(&p("me", 0)), "me", now), Holder::Mine);
        assert_eq!(holder(Some(&p("them", now - 3 * 60_000)), "me", now), Holder::Other { machine: "iMac".into(), age_ms: 3 * 60_000 });
        assert!(matches!(holder(Some(&p("them", now - 11 * 60_000)), "me", now), Holder::Stale { .. }));
        assert_eq!(ago(3 * 60_000), "3m ago");
    }

    #[test]
    fn tag_roundtrip() {
        let fx = Fixture::new("presence");
        let a = fx.clone("a");
        let g = fx.git(&a);
        let head = g.rev("HEAD").unwrap();
        let sha = make_tag(&g, &head, "MacBook Pro", "abc-123", 1_800_000_000).unwrap();
        let p = read(&g, &sha).unwrap();
        assert_eq!((p.machine.as_str(), p.install.as_str(), p.at_ms), ("MacBook Pro", "abc-123", 1_800_000_000_000));
    }
}
