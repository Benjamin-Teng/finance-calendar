//! 版本比較（SemVer 2.0 優先序）。外掛預設就只在「遠端版本 > 目前版本」時回傳更新；這裡再自己比一次當
//! 深度防禦（spec「MUST NOT 自動降版」），也讓 `update-state.json` 的退避判斷能比較版本。不引入 `semver`
//! 直接依賴（外掛內部已用，宿主只需要 x.y.z[-pre] 的比較）。

use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Vec<Ident>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Ident {
    Num(u64),
    Text(String),
}

impl Ord for Ident {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Num(a), Self::Num(b)) => a.cmp(b),
            (Self::Num(_), Self::Text(_)) => Ordering::Less,
            (Self::Text(_), Self::Num(_)) => Ordering::Greater,
            (Self::Text(a), Self::Text(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for Ident {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                // 有預發行標籤的版本低於同號的正式版。
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => self.pre.cmp(&other.pre),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Version {
    /// 解析 `x.y.z`、`vx.y.z`、`x.y.z-pre.1`、`x.y.z+build`；不合格式回 `None`。
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut nums = core.split('.');
        let major = nums.next()?.parse().ok()?;
        let minor = nums.next()?.parse().ok()?;
        let patch = nums.next()?.parse().ok()?;
        if nums.next().is_some() {
            return None;
        }
        let pre = match pre {
            None => Vec::new(),
            Some("") => return None,
            Some(p) => p
                .split('.')
                .map(|id| {
                    if id.is_empty() {
                        None
                    } else if id.chars().all(|c| c.is_ascii_digit()) {
                        id.parse().ok().map(Ident::Num)
                    } else {
                        Some(Ident::Text(id.to_owned()))
                    }
                })
                .collect::<Option<Vec<_>>>()?,
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

/// `candidate` 是否嚴格高於 `current`。任一邊解析不出來＝不是更新（寧可不更新，也不誤裝）。
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (Version::parse(current), Version::parse(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_only_when_strictly_higher() {
        assert!(is_newer("0.1.0", "0.1.1"));
        assert!(is_newer("0.1.9", "0.2.0"));
        assert!(is_newer("0.9.9", "1.0.0"));
        assert!(is_newer("0.1.0", "v0.1.1"));
        assert!(!is_newer("0.1.1", "0.1.1"), "同版本不是更新");
        assert!(!is_newer("0.1.1", "0.1.0"), "不得降版");
        assert!(!is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.10.0", "0.9.0"), "數字比較而非字串比較");
        assert!(is_newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn prerelease_orders_below_release() {
        assert!(is_newer("0.1.0-rc.1", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.0-rc.1"));
        assert!(is_newer("0.1.0-rc.1", "0.1.0-rc.2"));
        assert!(
            is_newer("0.1.0-rc.2", "0.1.0-rc.10"),
            "預發行數字識別字依數值比較"
        );
        assert!(
            is_newer("0.1.0-1", "0.1.0-alpha"),
            "數字識別字低於文字識別字"
        );
        assert!(is_newer("0.1.0-alpha", "0.1.0-alpha.1"));
    }

    #[test]
    fn build_metadata_is_ignored_and_garbage_is_not_newer() {
        assert!(!is_newer("0.1.0+a", "0.1.0+b"));
        assert!(!is_newer("0.1.0", "not-a-version"));
        assert!(!is_newer("garbage", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2"));
        assert!(!is_newer("0.1.0", "0.2.0.1"));
        assert!(!is_newer("0.1.0", "0.2.0-"));
    }
}
