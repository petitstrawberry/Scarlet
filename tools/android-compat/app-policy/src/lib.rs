//! Userspace translation of the pinned Android seapp selectors. This selects
//! exact names; registration and authorization remain native Shield operations.
#![no_std]
extern crate alloc;
use alloc::{
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec::Vec,
};

#[derive(Clone, Default)]
struct Rule {
    system: bool,
    ephemeral: Option<bool>,
    user: Option<String>,
    seinfo: Option<String>,
    name: Option<String>,
    privileged: Option<bool>,
    minimum: u32,
    run_as: bool,
    domain: Option<String>,
    file: Option<String>,
    level: String,
}
fn boolean(value: &str) -> Result<bool, &'static str> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("seapp boolean"),
    }
}
fn token(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn pattern(value: &str) -> bool {
    let value = value.strip_suffix('*').unwrap_or(value);
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:".contains(&b))
}
fn matches(pattern: &Option<String>, value: &str) -> bool {
    pattern
        .as_ref()
        .is_none_or(|pattern| match pattern.strip_suffix('*') {
            Some(prefix) => value.starts_with(prefix),
            None => pattern == value,
        })
}
fn precedence(value: &Option<String>) -> [u32; 3] {
    value.as_ref().map_or([0; 3], |value| {
        [
            1,
            u32::from(!value.ends_with('*')),
            if value.ends_with('*') {
                value.len() as u32
            } else {
                0
            },
        ]
    })
}
impl Rule {
    fn parse(line: &str) -> Result<Self, &'static str> {
        let mut rule = Self {
            level: "none".into(),
            ..Default::default()
        };
        let mut seen = BTreeSet::new();
        for word in line.split_ascii_whitespace() {
            let (key, value) = word.split_once('=').ok_or("seapp field")?;
            let key = key.to_ascii_lowercase();
            let value = value.to_ascii_lowercase();
            if !seen.insert(key.clone()) {
                return Err("duplicate seapp field");
            }
            match key.as_str() {
                "issystemserver" => rule.system = boolean(&value)?,
                "isephemeralapp" => rule.ephemeral = Some(boolean(&value)?),
                "isprivapp" => rule.privileged = Some(boolean(&value)?),
                "fromrunas" => rule.run_as = boolean(&value)?,
                "user" | "name" => {
                    if !pattern(&value) {
                        return Err("seapp pattern");
                    }
                    if key == "user" {
                        rule.user = Some(value)
                    } else {
                        rule.name = Some(value)
                    }
                }
                "seinfo" => {
                    if !token(&value) {
                        return Err("seapp signer selector");
                    }
                    rule.seinfo = Some(value);
                }
                "domain" | "type" => {
                    if !token(&value) {
                        return Err("seapp output");
                    }
                    if key == "domain" {
                        rule.domain = Some(value)
                    } else {
                        rule.file = Some(value)
                    }
                }
                "mintargetsdkversion" => {
                    rule.minimum = value.parse().map_err(|_| "seapp SDK")?;
                    if rule.minimum > i32::MAX as u32 {
                        return Err("seapp SDK range");
                    }
                }
                "levelfrom" => {
                    if !["none", "app", "user", "all"].contains(&value.as_str()) {
                        return Err("seapp level source");
                    }
                    rule.level = value;
                }
                "level" => {
                    if value != "s0" {
                        return Err("unsupported seapp level");
                    }
                }
                _ => return Err("unsupported seapp selector"),
            }
        }
        if rule.domain.is_none() && rule.file.is_none() {
            return Err("seapp output missing");
        }
        Ok(rule)
    }
    // Android 13's selector order: system, ephemeral, user, seinfo, package,
    // privileged status, minimum target SDK, then run-as.
    fn priority(&self) -> [u32; 11] {
        let user = precedence(&self.user);
        let name = precedence(&self.name);
        [
            u32::from(self.system),
            u32::from(self.ephemeral.is_some()),
            user[0],
            user[1],
            user[2],
            u32::from(self.seinfo.is_some()),
            name[0],
            name[1],
            name[2],
            u32::from(self.privileged.is_some()),
            self.minimum,
        ]
    }
}
pub struct Contexts(Vec<Rule>);
pub struct Mapping {
    pub uid: u32,
    pub domain: String,
    pub file: String,
}
impl Contexts {
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > 128 * 1024 {
            return Err("seapp input bound");
        }
        let mut rules = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            if line.len() > 2048 || rules.len() >= 256 {
                return Err("seapp rule bound");
            }
            rules.push(Rule::parse(line)?);
        }
        if rules.is_empty() {
            return Err("empty seapp input");
        }
        Ok(Self(rules))
    }
    pub fn classify(&self, name: &str, uid: u32, seinfo: &str) -> Result<Mapping, &'static str> {
        if name.len() > 255
            || name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._".contains(&b))
            || seinfo.len() > 512
        {
            return Err("package identity syntax");
        }
        let user_id = uid / 100000;
        let raw_app = uid % 100000;
        let (user, app) = match raw_app {
            0 => ("root", 0),
            1000 => ("system", 1000),
            1001 => ("radio", 1001),
            1002 => ("bluetooth", 1002),
            1027 => ("nfc", 1027),
            1068 => ("secure_element", 1068),
            1073 => ("network_stack", 1073),
            2000 => ("shell", 2000),
            10000..=19999 => ("_app", raw_app - 10000),
            20000..=89999 => ("_sdksandbox", raw_app - 20000),
            90000..=99999 => ("_isolated", raw_app - 90000),
            _ => return Err("unmapped Android AID"),
        };
        if app >= 65536 || user_id >= 65536 {
            return Err("seapp category range");
        }
        let name = name.to_ascii_lowercase();
        let seinfo = seinfo.to_ascii_lowercase();
        let mut parts = seinfo.split(':');
        let signer = parts.next().ok_or("seinfo signer")?;
        if !token(signer) {
            return Err("seinfo signer");
        }
        let mut privileged = false;
        let mut ephemeral = false;
        let mut run_as = false;
        let mut target = 0;
        let mut selectors = BTreeSet::new();
        for part in parts {
            let key = part.split('=').next().unwrap();
            if !selectors.insert(key) {
                return Err("duplicate seinfo token");
            }
            match part {
                "privapp" => privileged = true,
                "ephemeralapp" => ephemeral = true,
                "fromrunas" => run_as = true,
                "complete" => {}
                _ => {
                    let version = part
                        .strip_prefix("targetsdkversion=")
                        .ok_or("unsupported seinfo token")?;
                    target = version.parse::<u32>().map_err(|_| "package SDK")?;
                    if target > i32::MAX as u32 {
                        return Err("package SDK range");
                    }
                }
            }
        }
        let select = |file: bool| -> Result<String, &'static str> {
            let mut selected: Option<(&Rule, [u32; 11])> = None;
            for rule in &self.0 {
                let output = if file { &rule.file } else { &rule.domain };
                if rule.system
                    || output.is_none()
                    || rule.ephemeral.is_some_and(|value| value != ephemeral)
                    || rule.privileged.is_some_and(|value| value != privileged)
                    || rule.run_as != run_as
                    || rule.minimum > target
                    || !matches(&rule.user, user)
                    || !matches(&rule.seinfo, signer)
                    || !matches(&rule.name, &name)
                {
                    continue;
                }
                let priority = rule.priority();
                if let Some((previous, key)) = selected {
                    if key > priority {
                        continue;
                    }
                    if key == priority {
                        let previous_output = if file {
                            &previous.file
                        } else {
                            &previous.domain
                        };
                        if output != previous_output || rule.level != previous.level {
                            return Err("ambiguous seapp outputs");
                        }
                        continue;
                    }
                }
                selected = Some((rule, priority));
            }
            let (rule, _) = selected.ok_or("no delegated seapp rule")?;
            let range = match rule.level.as_str() {
                "none" => "s0".to_string(),
                "user" => format!(
                    "s0:c{},c{}",
                    512 + (user_id & 255),
                    768 + ((user_id >> 8) & 255)
                ),
                "app" => format!("s0:c{},c{}", app & 255, 256 + ((app >> 8) & 255)),
                "all" => format!(
                    "s0:c{},c{},c{},c{}",
                    app & 255,
                    256 + ((app >> 8) & 255),
                    512 + (user_id & 255),
                    768 + ((user_id >> 8) & 255)
                ),
                _ => return Err("unsupported seapp level"),
            };
            Ok(format!(
                "u:{}:{}:{}",
                if file { "object_r" } else { "r" },
                if file {
                    rule.file.as_ref()
                } else {
                    rule.domain.as_ref()
                }
                .unwrap(),
                range
            ))
        };
        Ok(Mapping {
            uid,
            domain: select(false)?,
            file: select(true)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const RULES: &str = "user=system seinfo=platform domain=system_app type=system_app_data_file\nuser=_app seinfo=platform domain=platform_app type=app_data_file levelFrom=user\nuser=_app isPrivApp=true name=com.android.special domain=special_app type=privapp_data_file levelFrom=all\nuser=_app isPrivApp=true domain=priv_app type=privapp_data_file levelFrom=user\nuser=_app minTargetSdkVersion=32 domain=untrusted_app type=app_data_file levelFrom=all\nuser=_app domain=untrusted_app_25 type=app_data_file levelFrom=user\n";
    #[test]
    fn stock_precedence_and_uid_categories_are_kept_separate() {
        let contexts = Contexts::parse(RULES).unwrap();
        let app = contexts
            .classify(
                "com.android.launcher3",
                10004,
                "platform:privapp:targetSdkVersion=33:complete",
            )
            .unwrap();
        assert_eq!(app.domain, "u:r:platform_app:s0:c512,c768");
        assert_eq!(app.file, "u:object_r:app_data_file:s0:c512,c768");
        let app = contexts
            .classify(
                "com.android.special",
                10004,
                "default:privapp:targetSdkVersion=33",
            )
            .unwrap();
        assert_eq!(app.domain, "u:r:special_app:s0:c4,c256,c512,c768");
        let app = contexts
            .classify("example.app", 110001, "default:targetSdkVersion=33")
            .unwrap();
        assert_eq!(app.domain, "u:r:untrusted_app:s0:c1,c256,c513,c768");
        assert_eq!(
            contexts
                .classify("android", 1000, "platform:privapp:targetSdkVersion=33")
                .unwrap()
                .domain,
            "u:r:system_app:s0"
        );
    }
    #[test]
    fn unsupported_ambiguous_and_malformed_inputs_fail_closed() {
        assert!(Contexts::parse("user=_app surprise=yes domain=app").is_err());
        assert!(Contexts::parse("user=_app user=system domain=app").is_err());
        let contexts = Contexts::parse(RULES).unwrap();
        for (uid, seinfo) in [
            (0, "platform"),
            (1, "default"),
            (89999, "default"),
            (10000, "default:targetSdkVersion=-1"),
            (10000, "default:unknown"),
        ] {
            assert!(contexts.classify("example.app", uid, seinfo).is_err());
        }
        let duplicate =
            Contexts::parse("user=_app domain=one type=data\nuser=_app domain=two type=data")
                .unwrap();
        assert!(duplicate.classify("example.app", 10000, "default").is_err());
    }
}
